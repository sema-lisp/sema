# Debug Adapter Protocol (DAP) Design Spec

**Goal:** Enable step debugging of Sema programs in IDEs (IntelliJ, VS Code, Neovim) via the standard Debug Adapter Protocol.

**Status:** Design — not yet implemented.

---

## 1. Overview

DAP support gives Sema users interactive debugging: set breakpoints, step through code line-by-line, inspect variables, and view the call stack — all from their IDE. This is the standard protocol used by VS Code, IntelliJ (via LSP4IJ/DAP4IJ), Neovim (nvim-dap), and others.

The Sema debugger will target the **bytecode VM** as the primary backend, since it has the richest existing plumbing (PC-indexed spans, local names, structured call frames). Tree-walker support is a future consideration.

---

## 2. Architecture

Mirror the LSP server pattern: async protocol frontend → channel → backend thread owning all `Rc`-based state.

```
┌─────────────────────────────────────────────────────────┐
│  IDE (VS Code / IntelliJ / Neovim)                      │
│  ← DAP protocol over stdio →                            │
└──────────────────────┬──────────────────────────────────┘
                       │ stdin/stdout (JSON-RPC)
┌──────────────────────▼──────────────────────────────────┐
│  sema-dap crate                                          │
│                                                          │
│  ┌─────────────────────┐    ┌─────────────────────────┐ │
│  │  Async Frontend      │    │  Backend Thread          │ │
│  │  (tokio)             │    │  (std::thread, owns Rc)  │ │
│  │                      │    │                          │ │
│  │  - Parse DAP JSON    │───►│  - Interpreter + VM      │ │
│  │  - Send responses    │◄───│  - DebugState            │ │
│  │  - Send events       │    │  - Breakpoint manager    │ │
│  │                      │    │  - Variable inspector    │ │
│  └─────────────────────┘    └─────────────────────────┘ │
│                                                          │
│  Channel: mpsc (Frontend→Backend requests)               │
│  Channel: mpsc (Backend→Frontend events/responses)       │
└──────────────────────────────────────────────────────────┘
```

### New crate: `sema-dap`

Dependencies: `sema-core`, `sema-reader`, `sema-eval`, `sema-vm`, `tokio`, `serde`, `serde_json`.

No external DAP library needed — the protocol is simple JSON-RPC over stdio with a `Content-Length` header (same framing as LSP). We can implement it directly.

### CLI entry point

```
sema dap          # launch DAP server on stdio
sema dap --port   # future: TCP transport
```

---

## 3. VM Debug Hooks

### Design principle: zero-cost when not debugging

The VM's hot loop must not pay any overhead when debugging is disabled. The debug hook is gated on a single pointer check.

### DebugState struct

```rust
pub struct DebugState {
    /// Active breakpoints: (file_path, line) → breakpoint ID
    pub breakpoints: HashMap<(PathBuf, u32), u32>,

    /// Current step mode
    pub step_mode: StepMode,

    /// Frame depth at which stepping was initiated (for StepOver/StepOut)
    pub step_frame_depth: usize,

    /// Last source line we stopped at (to avoid stopping on same line repeatedly)
    pub last_stop_line: Option<(PathBuf, u32)>,

    /// External pause request (set by DAP frontend, checked by VM)
    pub pause_requested: bool,

    /// Channel to send StoppedEvent to frontend
    pub event_tx: mpsc::Sender<DebugEvent>,

    /// Channel to receive Continue/Step commands from frontend
    pub command_rx: mpsc::Receiver<DebugCommand>,
}

pub enum StepMode {
    Continue,     // run until breakpoint
    StepInto,     // stop at next statement (any frame)
    StepOver,     // stop at next statement in same or parent frame
    StepOut,      // stop when returning to parent frame
}

pub enum DebugCommand {
    Continue,
    StepInto,
    StepOver,
    StepOut,
    Pause,
    SetBreakpoints { file: PathBuf, lines: Vec<u32>, reply: oneshot::Sender<Vec<u32>> },
    GetStackTrace { reply: oneshot::Sender<Vec<DapStackFrame>> },
    GetScopes { frame_id: usize, reply: oneshot::Sender<Vec<DapScope>> },
    GetVariables { scope_id: usize, reply: oneshot::Sender<Vec<DapVariable>> },
    Disconnect,
}

pub enum DebugEvent {
    Stopped { reason: StopReason, thread_id: u64 },
    Terminated,
    Output { category: String, output: String },
}
```

### VM integration point

Insert a debug check at **statement boundaries** — when the source span changes. This is the natural point because:
- Sema is a Lisp: each top-level form/expression is a "statement"
- The sparse `Chunk.spans` table records span changes, not every opcode
- Users think in terms of source lines, not opcodes

```rust
// In VM::run(), add to the `fn run()` signature:
pub fn run(&mut self, ctx: &EvalContext, debug: Option<&mut DebugState>) -> Result<Value, SemaError> {
    // ...
    loop {
        let op = unsafe { *code.add(pc) };
        pc += 1;

        // Debug hook: check at span boundaries (when source location changes)
        if let Some(ref mut dbg) = debug {
            // Only check when PC crosses a span boundary (cheap: binary search in sparse table)
            if let Some(span) = self.lookup_span_at_pc(&self.frames[fi].closure.func.chunk, pc) {
                if dbg.should_stop(span, self.frames.len()) {
                    self.frames[fi].pc = pc;
                    dbg.handle_stop(self, fi)?;
                    // handle_stop blocks on command_rx, updates step_mode, then returns
                }
            }
        }

        match op { ... }
    }
}
```

### Zero-cost path

When `debug` is `None` (normal execution), the `if let Some(...)` is a single pointer/option check — effectively free. The compiler will optimize this to a branch-not-taken prediction.

For maximum performance, we could also use a compile-time feature flag (`#[cfg(feature = "debug")]`) but the runtime check is simpler and sufficient given the VM already has branches for exception handling.

---

## 4. Breakpoints

### Source-level to PC mapping

Breakpoints are set by (file, line). To resolve to PC offsets:

1. When a program is compiled, each `Chunk` gets a sparse `spans: Vec<(u32, Span)>` table
2. To find the PC for line N: binary search `spans` for the first entry where `span.line == N`
3. If no exact match, use the next span after line N (first executable statement on or after that line)

```rust
fn resolve_breakpoint(chunk: &Chunk, file: &Path, line: u32) -> Option<u32> {
    chunk.spans.iter()
        .find(|(_, span)| span.line == line || span.line > line)
        .map(|(pc, _)| *pc)
}
```

### Breakpoint verification

DAP requires the server to respond with "verified" breakpoints and their actual line numbers. If a breakpoint is set on a blank/comment line, we snap to the next executable line and report the adjusted location.

### Multi-file support

The current VM doesn't track which file a chunk belongs to. We need to either:
- Add `file: Option<PathBuf>` to `Function`, or
- Use `EvalContext.current_file` to associate functions with files during compilation

**Recommendation:** Add `source_file: Option<PathBuf>` to `Function`.

---

## 5. Variable Inspection

### Locals

The VM stack holds local variables at `stack[base..base+n_locals]`. The `Function.local_names: Vec<(u16, Spur)>` maps slot indices to names.

```rust
fn inspect_locals(vm: &VM, frame_idx: usize) -> Vec<(String, Value)> {
    let frame = &vm.frames[frame_idx];
    let func = &frame.closure.func;
    let base = frame.base;
    func.local_names.iter().map(|(slot, spur)| {
        let name = resolve_spur(*spur);
        let value = vm.stack[base + *slot as usize].clone();
        (name, value)
    }).collect()
}
```

### Upvalues (captured variables)

Closures capture variables via `UpvalueCell`. The closure's `upvalues: Vec<Rc<UpvalueCell>>` can be read:

```rust
fn inspect_upvalues(vm: &VM, frame_idx: usize) -> Vec<(String, Value)> {
    let frame = &vm.frames[frame_idx];
    let closure = &frame.closure;
    closure.upvalues.iter().enumerate().map(|(i, cell)| {
        let name = format!("upvalue_{}", i); // TODO: preserve upvalue names in Function
        let value = cell.value.borrow().clone();
        (name, value)
    }).collect()
}
```

**Future improvement:** Preserve upvalue names in `Function` (currently only `UpvalueDesc` indices are stored).

### Globals

Read from `Env.bindings` (the `hashbrown::HashMap<Spur, Value>`). Too many to list by default — expose as a separate "Globals" scope that clients can expand lazily.

---

## 6. Call Stack

Translate VM `frames` to DAP `StackFrame` objects:

```rust
fn build_stack_trace(vm: &VM) -> Vec<DapStackFrame> {
    vm.frames.iter().rev().enumerate().map(|(id, frame)| {
        let func = &frame.closure.func;
        let name = func.name
            .map(|s| resolve_spur(s))
            .unwrap_or_else(|| "<lambda>".to_string());

        // Look up source location from PC
        let (line, col) = func.chunk.spans.iter()
            .rev()
            .find(|(pc, _)| *pc <= frame.pc as u32)
            .map(|(_, span)| (span.line as u64, span.col as u64))
            .unwrap_or((0, 0));

        DapStackFrame {
            id: id as u64,
            name,
            line,
            column: col,
            source: func.source_file.clone(), // needs to be added to Function
        }
    }).collect()
}
```

---

## 7. Scopes & Variables

DAP organizes variables into scopes. For each stack frame, expose:

| Scope | Contents |
|-------|----------|
| **Locals** | `stack[base..base+n_locals]` mapped via `local_names` |
| **Upvalues** | `closure.upvalues[..]` (captured variables) |
| **Globals** | `Env.bindings` (lazy-loaded, potentially large) |

Each scope gets a unique `variablesReference` ID. When the client requests variables for a scope, we read from the appropriate source.

### Structured values

For compound values (lists, maps, vectors), return them as structured variables with their own `variablesReference` so the client can expand them:
- List: children are `[0]`, `[1]`, `[2]`, ...
- Map: children are key→value pairs
- Vector: same as list
- Closure: show `<closure:name>` with arity info

---

## 8. Stepping

### StepInto
Stop at the next span change in any frame. This is the simplest: after every opcode, if the current span differs from `last_stop_span`, stop.

### StepOver
Stop at the next span change in the **same frame or a parent frame**. Record `step_frame_depth = frames.len()` when stepping. Stop when `frames.len() <= step_frame_depth` AND span changed.

### StepOut
Stop when `frames.len() < step_frame_depth` (i.e., after RETURN from the current frame).

### Tail calls
`TAIL_CALL` reuses the current frame, so `frames.len()` stays the same. StepInto works naturally. StepOver sees the frame depth unchanged, which is correct — the tail-called function is logically "replacing" the current one.

---

## 9. Tree-Walker Support

The tree-walker evaluator has a natural hook point: the `eval_step` trampoline in `eval.rs`. Each `Trampoline::Continue` iteration is one evaluation step.

**Recommendation:** DAP v1 targets **VM only**. The tree-walker can be added later by:
1. Adding a similar `DebugState` check in the trampoline loop
2. Using `EvalContext.call_stack` for frame inspection
3. Using `EvalContext.span_table` for source mapping

This is lower priority because:
- The VM is the production execution path
- The tree-walker's `Rc<Vec<Value>>` AST structure makes variable inspection harder (no slot-indexed locals)

---

## 10. DAP Protocol — v1 Scope

### Required requests (v1)

| Request | Implementation |
|---------|---------------|
| `initialize` | Return capabilities (supportsConfigurationDoneRequest, etc.) |
| `launch` | Parse source file, compile to bytecode, set up VM |
| `setBreakpoints` | Resolve source lines to PCs, store in DebugState |
| `configurationDone` | Start execution (VM runs until breakpoint/end) |
| `threads` | Return single thread (Sema is single-threaded) |
| `stackTrace` | Read VM frames, map to source locations |
| `scopes` | Return Locals/Upvalues/Globals for a frame |
| `variables` | Read values from stack/upvalues/env |
| `continue` | Set StepMode::Continue, unblock VM |
| `next` (step over) | Set StepMode::StepOver, record frame depth |
| `stepIn` | Set StepMode::StepInto |
| `stepOut` | Set StepMode::StepOut |
| `pause` | Set `pause_requested = true` |
| `disconnect` | Terminate execution, clean up |

### Required events (v1)

| Event | When |
|-------|------|
| `initialized` | After `initialize` response |
| `stopped` | VM hits breakpoint/step/pause |
| `terminated` | Program execution finished |
| `output` | stdout/stderr from the running program |

### Capabilities to advertise

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

---

## 11. CLI Integration

```rust
// In crates/sema/src/main.rs
Commands::Dap => {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(sema_dap::run_server())
}
```

### Launch configuration (VS Code example)

```json
{
  "type": "sema",
  "request": "launch",
  "name": "Debug Sema Program",
  "program": "${file}",
  "semaPath": "sema"
}
```

---

## 12. Implementation Order

1. **Add `source_file` to `Function`** — propagate file path through compiler
2. **Implement `DebugState`** — breakpoint set, step mode, channels
3. **Add debug hook to VM `run()`** — span-boundary check with zero-cost when disabled
4. **Implement DAP protocol handler** — JSON framing, request/response dispatch
5. **Wire it together** — `sema dap` subcommand, launch/attach lifecycle
6. **Variable inspection** — locals, upvalues, globals
7. **Test with VS Code** — create a basic launch configuration

---

## 13. Future Enhancements

- **Conditional breakpoints** — evaluate a Sema expression at the breakpoint, only stop if truthy
- **Logpoints** — evaluate and print an expression without stopping
- **Watch expressions** — evaluate arbitrary Sema expressions in the current scope
- **Exception breakpoints** — stop on `throw` / unhandled exceptions
- **Hot code reload** — recompile changed functions without restarting
- **Attach mode** — attach to a running `sema` process (requires IPC)
- **Tree-walker backend** — add debug hooks to the trampoline evaluator
- **Bytecode breakpoint section** — implement the reserved section 0x12 for persisted breakpoints
- **setVariable** — allow modifying locals/globals during debugging
- **Hover evaluation** — evaluate expressions on hover (integrate with LSP hover)
