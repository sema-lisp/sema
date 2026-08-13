# DAP (Debug Adapter Protocol) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable step debugging of Sema programs in IDEs via the Debug Adapter Protocol, targeting the bytecode VM.

**Architecture:** Async DAP protocol frontend (tokio, stdio JSON-RPC with `Content-Length` framing) communicates via `mpsc` channels with a backend `std::thread` that owns all `Rc`-based VM state. Mirrors the existing `sema-lsp` pattern. The VM's `run()` method gains an optional `DebugHook` callback checked at span boundaries.

**Tech Stack:** `tokio` (async I/O), `serde`/`serde_json` (DAP protocol serialization), `sema-vm` (bytecode VM), existing workspace crates.

**Design spec:** `docs/plans/2026-02-25-dap-debug-adapter.md`

---

## Phase 1: VM Foundations (Span propagation + Debug hook)

### Task 1: Propagate source spans through the VM compilation pipeline

Currently, `Chunk.spans` is always empty — the compiler never calls `Emitter::emit_span()`. The lowerer discards span info. We need spans in chunks before breakpoints or stepping can work.

**Files:**
- Modify: `crates/sema-vm/src/core_expr.rs` — add `span: Option<Span>` to `CoreExpr` and `ResolvedExpr` variants that map to statements/expressions
- Modify: `crates/sema-vm/src/lower.rs` — carry spans from parsed `Value` AST nodes into `CoreExpr` using the `SpanMap`
- Modify: `crates/sema-vm/src/resolve.rs` — pass spans through resolution
- Modify: `crates/sema-vm/src/compiler.rs` — call `self.emit.emit_span()` when compiling nodes that have spans
- Test: `crates/sema/tests/dual_eval_test.rs`

**Approach:** Rather than adding `span` to every `CoreExpr` variant (which would be invasive), wrap the expression tree with span annotations. Add a new variant `CoreExpr::Spanned(Span, Box<CoreExpr>)` and `ResolvedExpr::Spanned(Span, Box<ResolvedExpr>)`. The lowerer wraps top-level and significant expressions. The compiler emits spans when it encounters `Spanned` wrappers.

**Step 1: Add Spanned variant to CoreExpr**

In `crates/sema-vm/src/core_expr.rs`, add to the end of `enum CoreExpr`:
```rust
/// Source location annotation (transparent to evaluation)
Spanned(Span, Box<CoreExpr>),
```

And to `enum ResolvedExpr`:
```rust
/// Source location annotation (transparent to evaluation)
Spanned(Span, Box<ResolvedExpr>),
```

Import `Span` from `sema_core` at the top of the file.

**Step 2: Handle Spanned in the lowerer**

In `crates/sema-vm/src/lower.rs`:
- Change the `lower` function signature to accept an optional `&SpanMap` parameter, or better yet add a new entry point `lower_with_spans(val: &Value, span_map: &SpanMap) -> Result<CoreExpr, SemaError>`.
- In `lower_list()` (or the main dispatch), look up the `Value`'s pointer in the `SpanMap`. If found, wrap the resulting `CoreExpr` in `CoreExpr::Spanned(span, Box::new(inner))`.
- The span lookup uses `val.span_ptr()` or the raw pointer identity: `std::ptr::from_ref(val) as usize` won't work because `Value` is a u64 struct. The SpanMap keys are `usize` pointers to heap-allocated cons cells. Check how the tree-walker does span lookup (via `Value`'s inner pointer for list/cons values).

Actually, looking at how SpanMap is keyed: the reader stores `val.as_ptr()` or similar — let me trace this:

The reader does `self.span_map.insert(ptr, span)` where `ptr` is derived from the Value's heap allocation. `Value` stores list data via `Rc<Vec<Value>>`. The pointer identity is the `Rc`'s inner pointer address.

The VM lowerer (`lower.rs`) receives `&Value` and can obtain the same pointer identity to look up in the SpanMap. Add a helper to `Value` or use existing `.list_ptr()` / address-of-heap-data method. Check `crates/sema-core/src/value.rs` for how pointer identity is extracted for span tracking.

**Step 3: Handle Spanned in the resolver**

In `crates/sema-vm/src/resolve.rs`, add a match arm for `CoreExpr::Spanned(span, inner)`:
```rust
CoreExpr::Spanned(span, inner) => {
    let resolved_inner = resolve_expr(inner, ...)?;
    Ok(ResolvedExpr::Spanned(span, Box::new(resolved_inner)))
}
```

**Step 4: Emit spans in the compiler**

In `crates/sema-vm/src/compiler.rs`, add a match arm for `ResolvedExpr::Spanned(span, inner)`:
```rust
ResolvedExpr::Spanned(span, inner) => {
    self.emit.emit_span(*span);
    self.compile_expr(inner)?;
}
```

**Step 5: Wire up span propagation in compile_program**

In `crates/sema-vm/src/vm.rs`, modify `compile_program` to accept and forward a `SpanMap`:
```rust
pub fn compile_program_with_spans(vals: &[Value], span_map: &SpanMap) -> Result<(Rc<Closure>, Vec<Rc<Function>>), SemaError> {
    // use lower_with_spans instead of lower
}
```

Keep the old `compile_program` for backward compatibility (passes empty SpanMap).

**Step 6: Verify spans are populated**

Run: `cargo test -p sema-vm`
Expected: All existing tests pass (spans are optional, old paths don't break).

Write a unit test in `crates/sema-vm/src/vm.rs` tests module:
```rust
#[test]
fn test_spans_in_compiled_chunks() {
    let input = "(+ 1 2)\n(+ 3 4)";
    let (vals, span_map) = sema_reader::read_many_with_spans(input).unwrap();
    let (closure, _functions) = compile_program_with_spans(&vals, &span_map).unwrap();
    assert!(!closure.func.chunk.spans.is_empty(), "spans should be populated");
}
```

**Step 7: Commit**

```bash
git add crates/sema-vm/src/core_expr.rs crates/sema-vm/src/lower.rs crates/sema-vm/src/resolve.rs crates/sema-vm/src/compiler.rs crates/sema-vm/src/vm.rs
git commit -m "feat(vm): propagate source spans through compilation pipeline"
```

---

### Task 2: Add `source_file` to `Function`

The `Function` struct needs to know which source file it belongs to for multi-file debugging.

**Files:**
- Modify: `crates/sema-vm/src/chunk.rs` — add field
- Modify: `crates/sema-vm/src/compiler.rs` — propagate source_file through compilation
- Modify: `crates/sema-vm/src/vm.rs` — set source_file in `compile_program`

**Step 1: Add field to Function**

In `crates/sema-vm/src/chunk.rs`, add to `Function`:
```rust
pub source_file: Option<PathBuf>,
```
Add `use std::path::PathBuf;` at top.

**Step 2: Update all places that construct Function**

Search for `Function {` across the crate and add `source_file: None` to each construction site. Key locations:
- `crates/sema-vm/src/compiler.rs` — `Compiler::finish()` and any lambda compilation
- `crates/sema-vm/src/vm.rs` — `compile_program`
- `crates/sema-vm/src/serialize.rs` — deserialization

**Step 3: Verify**

Run: `cargo build`
Run: `cargo test -p sema-vm`
Expected: All pass.

**Step 4: Commit**

```bash
git add crates/sema-vm/src/chunk.rs crates/sema-vm/src/compiler.rs crates/sema-vm/src/vm.rs crates/sema-vm/src/serialize.rs
git commit -m "feat(vm): add source_file field to Function"
```

---

### Task 3: Add DebugHook to VM

The VM's `run()` loop needs a hook point for the debugger. This must be zero-cost when debugging is disabled.

**Files:**
- Create: `crates/sema-vm/src/debug.rs` — DebugState, StepMode, debug hook types
- Modify: `crates/sema-vm/src/vm.rs` — add debug hook parameter to `run()`, check at span boundaries
- Modify: `crates/sema-vm/src/lib.rs` — export debug module
- Test: `crates/sema-vm/src/vm.rs` (unit tests)

**Step 1: Create debug.rs with core types**

Create `crates/sema-vm/src/debug.rs`:
```rust
use std::path::PathBuf;
use std::sync::mpsc;
use sema_core::Span;

/// Current stepping mode for the debugger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepMode {
    /// Run until a breakpoint is hit.
    Continue,
    /// Stop at the next source line change (any frame depth).
    StepInto,
    /// Stop at the next source line change in the same or parent frame.
    StepOver,
    /// Stop when returning to the parent frame.
    StepOut,
}

/// Commands sent from the DAP frontend to the VM backend.
pub enum DebugCommand {
    Continue,
    StepInto,
    StepOver,
    StepOut,
    Pause,
    SetBreakpoints {
        file: PathBuf,
        lines: Vec<u32>,
        reply: mpsc::SyncSender<Vec<u32>>,
    },
    Disconnect,
}

/// Events sent from the VM backend to the DAP frontend.
pub enum DebugEvent {
    Stopped {
        reason: StopReason,
        description: Option<String>,
    },
    Terminated,
    Output {
        category: String,
        output: String,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum StopReason {
    Breakpoint,
    Step,
    Pause,
    Entry,
}

/// Mutable debugger state carried alongside the VM.
pub struct DebugState {
    /// Active breakpoints: (file_path, line) → breakpoint ID
    pub breakpoints: std::collections::HashMap<(PathBuf, u32), u32>,
    /// Current step mode
    pub step_mode: StepMode,
    /// Frame depth when stepping was initiated
    pub step_frame_depth: usize,
    /// Last source location we stopped at (file, line)
    pub last_stop_line: Option<(PathBuf, u32)>,
    /// External pause request (set by DAP frontend, checked by VM)
    pub pause_requested: bool,
    /// Channel to send events to the DAP frontend
    pub event_tx: mpsc::Sender<DebugEvent>,
    /// Channel to receive commands from the DAP frontend
    pub command_rx: mpsc::Receiver<DebugCommand>,
    next_bp_id: u32,
}

impl DebugState {
    pub fn new(
        event_tx: mpsc::Sender<DebugEvent>,
        command_rx: mpsc::Receiver<DebugCommand>,
    ) -> Self {
        DebugState {
            breakpoints: std::collections::HashMap::new(),
            step_mode: StepMode::Continue,
            step_frame_depth: 0,
            last_stop_line: None,
            pause_requested: false,
            event_tx,
            command_rx,
            next_bp_id: 1,
        }
    }

    /// Check if we should stop at the given span and frame depth.
    pub fn should_stop(&self, file: Option<&PathBuf>, line: u32, frame_depth: usize) -> bool {
        // Check pause request
        if self.pause_requested {
            return true;
        }

        // Check breakpoints
        if let Some(f) = file {
            if self.breakpoints.contains_key(&(f.clone(), line)) {
                return true;
            }
        }

        // Check step mode
        match self.step_mode {
            StepMode::Continue => false,
            StepMode::StepInto => {
                // Stop if line changed
                match &self.last_stop_line {
                    Some((_, last_line)) => line != *last_line,
                    None => true,
                }
            }
            StepMode::StepOver => {
                frame_depth <= self.step_frame_depth && match &self.last_stop_line {
                    Some((_, last_line)) => line != *last_line,
                    None => true,
                }
            }
            StepMode::StepOut => {
                frame_depth < self.step_frame_depth
            }
        }
    }

    /// Block the VM thread, send a Stopped event, and wait for a resume command.
    pub fn handle_stop(&mut self, file: Option<&PathBuf>, line: u32, frame_depth: usize, reason: StopReason) {
        self.last_stop_line = file.map(|f| (f.clone(), line));
        self.pause_requested = false;

        // Notify frontend
        let _ = self.event_tx.send(DebugEvent::Stopped {
            reason,
            description: None,
        });

        // Block until we get a resume command
        loop {
            match self.command_rx.recv() {
                Ok(DebugCommand::Continue) => {
                    self.step_mode = StepMode::Continue;
                    break;
                }
                Ok(DebugCommand::StepInto) => {
                    self.step_mode = StepMode::StepInto;
                    self.step_frame_depth = frame_depth;
                    break;
                }
                Ok(DebugCommand::StepOver) => {
                    self.step_mode = StepMode::StepOver;
                    self.step_frame_depth = frame_depth;
                    break;
                }
                Ok(DebugCommand::StepOut) => {
                    self.step_mode = StepMode::StepOut;
                    self.step_frame_depth = frame_depth;
                    break;
                }
                Ok(DebugCommand::Pause) => {
                    // Already paused
                }
                Ok(DebugCommand::SetBreakpoints { file, lines, reply }) => {
                    let ids = self.set_breakpoints(&file, &lines);
                    let _ = reply.send(ids);
                }
                Ok(DebugCommand::Disconnect) => {
                    self.step_mode = StepMode::Continue;
                    break;
                }
                Err(_) => {
                    // Frontend disconnected
                    self.step_mode = StepMode::Continue;
                    break;
                }
            }
        }
    }

    /// Set breakpoints for a file, replacing any existing ones for that file.
    pub fn set_breakpoints(&mut self, file: &PathBuf, lines: &[u32]) -> Vec<u32> {
        // Remove existing breakpoints for this file
        self.breakpoints.retain(|(f, _), _| f != file);

        // Add new breakpoints
        lines.iter().map(|&line| {
            let id = self.next_bp_id;
            self.next_bp_id += 1;
            self.breakpoints.insert((file.clone(), line), id);
            id
        }).collect()
    }
}
```

**Step 2: Add debug module to lib.rs**

In `crates/sema-vm/src/lib.rs`, add:
```rust
pub mod debug;
```
And add to exports:
```rust
pub use debug::{DebugCommand, DebugEvent, DebugState, StepMode, StopReason};
```

**Step 3: Add debug hook to VM::run()**

In `crates/sema-vm/src/vm.rs`, modify the `run` method signature:
```rust
fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
```
becomes:
```rust
fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
    self.run_inner(ctx, None)
}

fn run_debug(&mut self, ctx: &EvalContext, debug: &mut DebugState) -> Result<Value, SemaError> {
    self.run_inner(ctx, Some(debug))
}

fn run_inner(&mut self, ctx: &EvalContext, mut debug: Option<&mut DebugState>) -> Result<Value, SemaError> {
```

In the inner dispatch loop, after `let op = unsafe { *code.add(pc) }; pc += 1;`, add:
```rust
// Debug hook: check at span boundaries
if let Some(ref mut dbg) = debug {
    let chunk = &self.frames[fi].closure.func.chunk;
    // Binary search for span at current PC
    let pc32 = (pc - 1) as u32;
    if let Ok(idx) = chunk.spans.binary_search_by_key(&pc32, |(p, _)| *p) {
        let span = &chunk.spans[idx].1;
        let source_file = self.frames[fi].closure.func.source_file.as_ref();
        let line = span.line as u32;
        let frame_depth = self.frames.len();
        if dbg.should_stop(source_file, line, frame_depth) {
            self.frames[fi].pc = pc;
            let reason = if dbg.pause_requested {
                crate::debug::StopReason::Pause
            } else if dbg.step_mode != crate::debug::StepMode::Continue {
                crate::debug::StopReason::Step
            } else {
                crate::debug::StopReason::Breakpoint
            };
            dbg.handle_stop(source_file, line, frame_depth, reason);
        }
    }
}
```

Also add `execute_debug` method:
```rust
pub fn execute_debug(&mut self, closure: Rc<Closure>, ctx: &EvalContext, debug: &mut DebugState) -> Result<Value, SemaError> {
    let base = self.stack.len();
    let n_locals = closure.func.chunk.n_locals as usize;
    self.stack.resize(base + n_locals, Value::nil());
    self.frames.push(CallFrame {
        closure,
        pc: 0,
        base,
        open_upvalues: None,
    });
    self.run_debug(ctx, debug)
}
```

**Step 4: Verify nothing breaks**

Run: `cargo test -p sema-vm`
Run: `cargo test -p sema --test integration_test`
Expected: All existing tests pass (debug is always `None` in non-debug paths).

**Step 5: Write a basic debug hook test**

In `crates/sema-vm/src/vm.rs` tests:
```rust
#[test]
fn test_debug_hook_fires() {
    use std::sync::mpsc;
    use crate::debug::{DebugState, DebugCommand, DebugEvent, StepMode};

    let (event_tx, event_rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let mut debug = DebugState::new(event_tx, cmd_rx);
    debug.step_mode = StepMode::StepInto;

    // Immediately send Continue so the VM doesn't block
    cmd_tx.send(DebugCommand::Continue).unwrap();

    // This test validates the types compile and connect correctly.
    // Full integration testing requires spans to be populated (Task 1).
}
```

**Step 6: Commit**

```bash
git add crates/sema-vm/src/debug.rs crates/sema-vm/src/vm.rs crates/sema-vm/src/lib.rs
git commit -m "feat(vm): add DebugState and debug hook to VM run loop"
```

---

### Task 4: Add variable/stack inspection methods to VM

The DAP server needs to read locals, upvalues, and globals from the VM state. These methods should be on `VM` but only accessible when debugging.

**Files:**
- Modify: `crates/sema-vm/src/vm.rs` — add inspection methods
- Modify: `crates/sema-vm/src/debug.rs` — add DAP types (DapStackFrame, DapVariable, etc.)

**Step 1: Add DAP response types to debug.rs**

```rust
#[derive(Debug, Clone)]
pub struct DapStackFrame {
    pub id: u64,
    pub name: String,
    pub line: u64,
    pub column: u64,
    pub source_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DapVariable {
    pub name: String,
    pub value: String,
    pub type_name: String,
    /// Non-zero if this variable has children (structured: list, map, vector)
    pub variables_reference: u64,
}

#[derive(Debug, Clone)]
pub struct DapScope {
    pub name: String,
    pub variables_reference: u64,
    pub expensive: bool,
}
```

**Step 2: Add inspection methods to VM**

In `crates/sema-vm/src/vm.rs`, add `pub` methods:

```rust
/// Get the current call stack as DAP stack frames.
pub fn debug_stack_trace(&self) -> Vec<DapStackFrame> { ... }

/// Get locals for a given frame index.
pub fn debug_locals(&self, frame_idx: usize) -> Vec<DapVariable> { ... }

/// Get upvalues for a given frame index.
pub fn debug_upvalues(&self, frame_idx: usize) -> Vec<DapVariable> { ... }

/// Get the number of frames.
pub fn debug_frame_count(&self) -> usize { self.frames.len() }
```

Use `sema_core::pretty_print` to format values for display.

**Step 3: Verify**

Run: `cargo test -p sema-vm`
Expected: All pass.

**Step 4: Commit**

```bash
git add crates/sema-vm/src/vm.rs crates/sema-vm/src/debug.rs
git commit -m "feat(vm): add debug inspection methods for stack, locals, upvalues"
```

---

## Phase 2: DAP Protocol Server

### Task 5: Create sema-dap crate skeleton

**Files:**
- Create: `crates/sema-dap/Cargo.toml`
- Create: `crates/sema-dap/src/lib.rs`
- Create: `crates/sema-dap/src/protocol.rs` — DAP JSON message types
- Create: `crates/sema-dap/src/transport.rs` — Content-Length framed stdio I/O
- Modify: `Cargo.toml` (workspace) — add member

**Step 1: Create Cargo.toml**

```toml
[package]
name = "sema-dap"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
description = "Debug Adapter Protocol server for Sema"

[dependencies]
sema-core.workspace = true
sema-reader.workspace = true
sema-eval.workspace = true
sema-vm.workspace = true
sema-stdlib.workspace = true
tokio = { workspace = true, features = ["rt-multi-thread", "macros", "io-std", "sync", "io-util"] }
serde.workspace = true
serde_json.workspace = true
```

**Step 2: Add to workspace**

In root `Cargo.toml`, add `"crates/sema-dap"` to `members` list. Add `sema-dap = { version = "=1.12.0", path = "crates/sema-dap" }` to `[workspace.dependencies]`.

**Step 3: Create transport.rs — Content-Length framed I/O**

Implement `read_message` and `write_message` functions using `tokio::io::AsyncBufRead` / `AsyncWrite`. The framing is identical to LSP: `Content-Length: N\r\n\r\n{json}`.

```rust
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

pub async fn read_message(reader: &mut (impl AsyncBufRead + Unpin)) -> std::io::Result<Option<String>> {
    // Read headers
    let mut content_length: Option<usize> = None;
    let mut header = String::new();
    loop {
        header.clear();
        let n = reader.read_line(&mut header).await?;
        if n == 0 { return Ok(None); } // EOF
        let trimmed = header.trim();
        if trimmed.is_empty() { break; } // end of headers
        if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
            content_length = len_str.parse().ok();
        }
    }
    let len = content_length.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    Ok(Some(String::from_utf8_lossy(&body).into_owned()))
}

pub async fn write_message(writer: &mut (impl AsyncWrite + Unpin), body: &str) -> std::io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body.as_bytes()).await?;
    writer.flush().await
}
```

**Step 4: Create protocol.rs — DAP types**

Define the core DAP request/response/event types using serde. Implement only the v1 subset from the spec:
- `InitializeRequest` / `InitializeResponse`
- `LaunchRequest`
- `SetBreakpointsRequest` / `SetBreakpointsResponse`
- `ConfigurationDoneRequest`
- `ThreadsRequest` / `ThreadsResponse`
- `StackTraceRequest` / `StackTraceResponse`
- `ScopesRequest` / `ScopesResponse`
- `VariablesRequest` / `VariablesResponse`
- `ContinueRequest` / `ContinueResponse`
- `NextRequest` (step over)
- `StepInRequest`
- `StepOutRequest`
- `PauseRequest`
- `DisconnectRequest`
- Events: `initialized`, `stopped`, `terminated`, `output`

Use a generic `DapMessage` envelope:
```rust
#[derive(Debug, Deserialize)]
pub struct DapMessage {
    pub seq: u64,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub command: Option<String>,
    pub arguments: Option<serde_json::Value>,
    // for events
    pub event: Option<String>,
    pub body: Option<serde_json::Value>,
}
```

**Step 5: Create lib.rs**

```rust
pub mod protocol;
pub mod transport;

pub async fn run_server() {
    eprintln!("Sema DAP server starting on stdio...");
    // TODO: implement in next task
}
```

**Step 6: Verify**

Run: `cargo build -p sema-dap`
Expected: Compiles.

**Step 7: Commit**

```bash
git add crates/sema-dap/ Cargo.toml
git commit -m "feat: create sema-dap crate skeleton with protocol types and transport"
```

---

### Task 6: Implement DAP server main loop

Wire up the async frontend (reading/writing DAP messages on stdio) with the backend thread that owns the VM.

**Files:**
- Create: `crates/sema-dap/src/server.rs` — main server logic
- Modify: `crates/sema-dap/src/lib.rs` — wire up `run_server()`

**Step 1: Create server.rs**

Implement the DAP server following the LSP pattern:

```rust
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc as tokio_mpsc;
use sema_vm::debug::{DebugState, DebugCommand, DebugEvent};

/// Messages from the async frontend to the backend thread.
enum BackendRequest {
    Launch { program: PathBuf, reply: tokio_mpsc::Sender<Result<(), String>> },
    SetBreakpoints { file: PathBuf, lines: Vec<u32>, reply: tokio_mpsc::Sender<Vec<u32>> },
    Continue,
    StepIn,
    StepOver,
    StepOut,
    Pause,
    GetStackTrace { reply: tokio_mpsc::Sender<Vec<sema_vm::debug::DapStackFrame>> },
    GetScopes { frame_id: usize, reply: tokio_mpsc::Sender<Vec<sema_vm::debug::DapScope>> },
    GetVariables { reference: u64, reply: tokio_mpsc::Sender<Vec<sema_vm::debug::DapVariable>> },
    Disconnect,
}
```

The async `run_server()`:
1. Creates tokio stdin/stdout readers
2. Creates `tokio::sync::mpsc` channel for frontend→backend
3. Spawns a `std::thread` for the backend
4. In a select! loop: reads DAP messages from stdin, dispatches to backend, reads events from backend and sends DAP events/responses to stdout

The backend thread:
1. Receives `BackendRequest::Launch` → reads file, parses, compiles (with spans), creates VM
2. Creates `std::sync::mpsc` channels for DebugState ↔ VM communication
3. Runs VM with debug hook
4. Forwards DebugEvents back to frontend

**Step 2: Implement request handling**

For each DAP request, parse the arguments, forward to the backend, and send the appropriate DAP response.

The `initialize` request returns capabilities:
```json
{
    "supportsConfigurationDoneRequest": true,
    "supportsFunctionBreakpoints": false,
    "supportsConditionalBreakpoints": false,
    "supportsStepBack": false,
    "supportsSetVariable": false,
    "supportsRestartFrame": false,
    "supportsModulesRequest": false,
    "supportsExceptionInfoRequest": false
}
```

After sending the `initialize` response, send an `initialized` event.

The `launch` request reads `arguments.program`, forwards to the backend.

The `configurationDone` request signals the backend to start VM execution.

The `threads` request always returns a single thread `{ "id": 1, "name": "main" }`.

**Step 3: Verify**

Run: `cargo build -p sema-dap`
Expected: Compiles.

**Step 4: Commit**

```bash
git add crates/sema-dap/src/server.rs crates/sema-dap/src/lib.rs
git commit -m "feat(dap): implement DAP server main loop with frontend/backend architecture"
```

---

### Task 7: Add `sema dap` CLI subcommand

**Files:**
- Modify: `crates/sema/src/main.rs` — add `Dap` variant to `Commands` enum and handler
- Modify: `crates/sema/Cargo.toml` — add `sema-dap` dependency

**Step 1: Add dependency**

In `crates/sema/Cargo.toml`, add:
```toml
sema-dap.workspace = true
```

**Step 2: Add subcommand**

In `crates/sema/src/main.rs`, add to `enum Commands`:
```rust
/// Start the Debug Adapter Protocol server
Dap,
```

In the match on `command` in `main()`, add:
```rust
Commands::Dap => {
    eprintln!("Sema DAP server starting on stdio...");
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime")
        .block_on(sema_dap::run_server());
}
```

**Step 3: Verify**

Run: `cargo build -p sema-lang`
Run: `cargo run -- dap --help` (should print help or start server)
Expected: Compiles and runs.

**Step 4: Commit**

```bash
git add crates/sema/src/main.rs crates/sema/Cargo.toml Cargo.toml
git commit -m "feat: add 'sema dap' CLI subcommand"
```

---

## Phase 3: Integration & Testing

### Task 8: End-to-end integration test

Write an integration test that simulates a DAP client, sends initialize/launch/setBreakpoints/configurationDone, receives stopped events, and verifies stack trace + variable inspection.

**Files:**
- Create: `crates/sema-dap/tests/integration_test.rs`

**Step 1: Write test**

The test should:
1. Spawn the `sema dap` binary as a child process (or test `run_server` directly using in-process stdio pipes)
2. Send `initialize` request, verify response
3. Send `launch` with a simple .sema program
4. Send `setBreakpoints` for a specific line
5. Send `configurationDone`
6. Receive `stopped` event
7. Send `stackTrace`, verify frames
8. Send `scopes`, then `variables`, verify locals
9. Send `continue`
10. Receive `terminated` event
11. Send `disconnect`

**Step 2: Verify**

Run: `cargo test -p sema-dap`
Expected: Integration test passes.

**Step 3: Commit**

```bash
git add crates/sema-dap/tests/
git commit -m "test(dap): add end-to-end DAP integration test"
```

---

### Task 9: VS Code launch configuration

**Files:**
- Create: `editors/vscode/sema-debug/package.json` — VS Code debug extension stub
- Or document the launch configuration in the website/docs

**Step 1: Create launch.json example**

Add to `editors/vscode/` or docs:
```json
{
    "version": "0.2.0",
    "configurations": [
        {
            "type": "sema",
            "request": "launch",
            "name": "Debug Sema Program",
            "program": "${file}",
            "semaPath": "sema"
        }
    ]
}
```

**Step 2: Commit**

```bash
git add editors/ docs/
git commit -m "docs: add VS Code DAP launch configuration example"
```

---

## Summary of Implementation Order

| # | Task | Crate | Depends On |
|---|------|-------|------------|
| 1 | Propagate spans through VM compiler | sema-vm | — |
| 2 | Add source_file to Function | sema-vm | — |
| 3 | Add DebugHook to VM | sema-vm | 1, 2 |
| 4 | Variable/stack inspection methods | sema-vm | 3 |
| 5 | Create sema-dap crate skeleton | sema-dap | — |
| 6 | Implement DAP server main loop | sema-dap | 3, 4, 5 |
| 7 | Add `sema dap` CLI subcommand | sema | 5 |
| 8 | End-to-end integration test | sema-dap | 6, 7 |
| 9 | VS Code launch configuration | docs | 8 |

Tasks 1, 2, and 5 can be done in parallel. Tasks 3 and 4 are sequential after 1+2. Task 6 depends on 3+4+5. Tasks 7 and 8 are sequential after 6.
