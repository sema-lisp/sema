# Playground Debugger Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a lightweight visual debugger to the Sema playground: breakpoint gutter, step/continue buttons, current line highlighting, and variable inspection — all running in single-threaded WASM.

**Architecture:** Refactor the VM's `run_inner` to be cooperative (return `VmExecResult::Stopped` instead of blocking on channels), then build WASM bindings that expose a step-and-yield debug API, and a playground UI with gutter breakpoints + debug controls. The existing native DAP server becomes a blocking driver loop around the same cooperative `run_inner`.

**Tech Stack:** Rust (sema-vm, sema-wasm), wasm-bindgen, vanilla JS (playground), CSS

---

## Task 1: Add `VmExecResult` and `resume_skip` to debug.rs

**Files:**
- Modify: `crates/sema-vm/src/debug.rs`
- Modify: `crates/sema-vm/src/lib.rs` (re-exports)

**Step 1: Add VmExecResult enum and StopInfo struct**

Add at the end of `crates/sema-vm/src/debug.rs`:

```rust
/// Result of cooperative VM execution.
#[derive(Debug)]
pub enum VmExecResult {
    /// Execution completed normally with a return value.
    Finished(Value),
    /// Execution paused at a debug stop point.
    Stopped(StopInfo),
}

/// Information about why and where the VM stopped.
#[derive(Debug, Clone)]
pub struct StopInfo {
    pub reason: StopReason,
    pub file: Option<PathBuf>,
    pub line: u32,
}
```

Add `use sema_core::Value;` to the imports at the top of `debug.rs`.

**Step 2: Add `resume_skip` field to `DebugState`**

Add `pub resume_skip: bool` field to `DebugState` struct. Initialize it to `false` in `DebugState::new()`.

**Step 3: Add `DebugState::new_headless()` constructor**

Add a constructor that creates a DebugState without channels — for WASM use. Use dummy channels (create `mpsc::channel()` and immediately drop the sender/receiver... actually simpler: use `Option`).

Wait — instead of making channels optional (which changes many call sites), create `new_headless()` using disconnected channels:

```rust
/// Create a DebugState without functional channels.
/// Used for cooperative (WASM) execution where commands are applied
/// between `run_inner` calls, not via channels.
pub fn new_headless() -> Self {
    let (event_tx, _) = mpsc::channel();
    let (_, command_rx) = mpsc::channel();
    DebugState {
        breakpoints: std::collections::HashMap::new(),
        step_mode: StepMode::Continue,
        step_frame_depth: 0,
        last_stop_line: None,
        pause_requested: false,
        event_tx,
        command_rx,
        resume_skip: false,
        next_bp_id: 1,
    }
}
```

This is the simplest approach: `try_recv` on a disconnected channel returns `Err(TryRecvError::Disconnected)` which is handled by the existing `while let Ok(cmd)` pattern (it just falls through). `send` on the event channel silently fails (returns Err, which is ignored with `let _ =`).

**Step 4: Update re-exports in `crates/sema-vm/src/lib.rs`**

Add `VmExecResult` and `StopInfo` to the `pub use debug::` line.

**Step 5: Verify compilation**

Run: `cargo build -p sema-vm`
Expected: compiles with no errors (new types are unused but that's OK)

**Step 6: Commit**

```bash
git add crates/sema-vm/src/debug.rs crates/sema-vm/src/lib.rs
git commit -m "feat(vm): add VmExecResult, StopInfo, resume_skip for cooperative debug"
```

---

## Task 2: Refactor `run_inner` to be cooperative

This is the core change. Replace the blocking `command_rx.recv()` loop with a `return Ok(VmExecResult::Stopped(...))`.

**Files:**
- Modify: `crates/sema-vm/src/vm.rs`

**Step 1: Change `run_inner` return type**

Change:
```rust
fn run_inner(
    &mut self,
    ctx: &EvalContext,
    mut debug: Option<&mut crate::debug::DebugState>,
) -> Result<Value, SemaError> {
```
To:
```rust
fn run_inner(
    &mut self,
    ctx: &EvalContext,
    mut debug: Option<&mut crate::debug::DebugState>,
) -> Result<crate::debug::VmExecResult, SemaError> {
```

**Step 2: Update all `return Ok(...)` sites in `run_inner`**

There are 3 sites that return `Ok(...)` inside `run_inner`:

1. **Line ~244** (Disconnect during polling): Change `return Ok(Value::nil())` → `return Ok(crate::debug::VmExecResult::Finished(Value::nil()))`

2. **Line ~358** (Disconnect during stop): This will be removed in the next step (the blocking loop is going away).

3. **Line ~529** (RETURN opcode, frames empty): Change `return Ok(result)` → `return Ok(crate::debug::VmExecResult::Finished(result))`

**Step 3: Replace the blocking stop loop with cooperative return**

Replace the entire stop section (lines ~294-367) — the `if let Some((file, line)) = at_span { ... }` block — with:

```rust
if let Some((file, line)) = at_span {
    // Skip the first debug check after resume to avoid
    // re-triggering the same breakpoint
    if dbg.resume_skip {
        dbg.resume_skip = false;
    } else {
        let frame_depth = self.frames.len();
        if dbg.should_stop(file.as_ref(), line, frame_depth) {
            // Save PC at opcode position (pc-1) so we re-execute
            // this instruction on resume
            self.frames[fi].pc = pc - 1;
            let reason = if dbg.pause_requested {
                crate::debug::StopReason::Pause
            } else if dbg.step_mode != crate::debug::StepMode::Continue {
                crate::debug::StopReason::Step
            } else {
                crate::debug::StopReason::Breakpoint
            };
            dbg.last_stop_line = file.as_ref().map(|f| (f.clone(), line));
            dbg.pause_requested = false;
            dbg.resume_skip = true;
            return Ok(crate::debug::VmExecResult::Stopped(
                crate::debug::StopInfo {
                    reason,
                    file: file.clone(),
                    line,
                },
            ));
        }
    }
}
```

Key changes from the original:
- Save `pc - 1` (opcode position) instead of `pc` (post-opcode). This is correct because on resume, `run_inner` re-enters the `'dispatch` loop which reads `pc = frame.pc`, then the inner loop reads the opcode at that position.
- Set `resume_skip = true` before returning. On resume, the first debug check for this instruction is skipped (avoiding re-triggering breakpoints on the same line).
- Return `VmExecResult::Stopped` instead of blocking on `command_rx.recv()`.
- No `event_tx.send()` — the caller (WASM or native driver) handles notification.

**Step 4: Update `run()` to unwrap VmExecResult**

Change:
```rust
fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
    self.run_inner(ctx, None)
}
```
To:
```rust
fn run(&mut self, ctx: &EvalContext) -> Result<Value, SemaError> {
    match self.run_inner(ctx, None)? {
        crate::debug::VmExecResult::Finished(v) => Ok(v),
        crate::debug::VmExecResult::Stopped(_) => unreachable!("Stopped without debug state"),
    }
}
```

**Step 5: Delete `run_debug` method**

Remove the `run_debug` method entirely — it's no longer needed. `execute_debug` will call `run_inner` directly.

**Step 6: Verify compilation**

Run: `cargo build -p sema-vm`
Expected: Compile error in `execute_debug` (it calls `run_debug` which is deleted). That's expected — fixed in Task 3.

**Step 7: Commit** (will combine with Task 3)

---

## Task 3: Refactor `execute_debug` to be the blocking driver

**Files:**
- Modify: `crates/sema-vm/src/vm.rs`

**Step 1: Rewrite `execute_debug`**

Replace the current `execute_debug` method with a blocking driver loop that wraps the cooperative `run_inner`:

```rust
pub fn execute_debug(
    &mut self,
    closure: Rc<Closure>,
    ctx: &EvalContext,
    debug: &mut crate::debug::DebugState,
) -> Result<Value, SemaError> {
    let base = self.stack.len();
    let n_locals = closure.func.chunk.n_locals as usize;
    self.stack.resize(base + n_locals, Value::nil());
    self.frames.push(CallFrame {
        closure,
        pc: 0,
        base,
        open_upvalues: None,
    });

    loop {
        match self.run_inner(ctx, Some(debug))? {
            crate::debug::VmExecResult::Finished(v) => return Ok(v),
            crate::debug::VmExecResult::Stopped(info) => {
                // Send stopped event to DAP frontend
                let _ = debug.event_tx.send(crate::debug::DebugEvent::Stopped {
                    reason: info.reason,
                    description: None,
                });

                // Block waiting for commands from DAP frontend
                loop {
                    match debug.command_rx.recv() {
                        Ok(crate::debug::DebugCommand::Continue) => {
                            debug.step_mode = crate::debug::StepMode::Continue;
                            break;
                        }
                        Ok(crate::debug::DebugCommand::StepInto) => {
                            debug.step_mode = crate::debug::StepMode::StepInto;
                            debug.step_frame_depth = self.frames.len();
                            break;
                        }
                        Ok(crate::debug::DebugCommand::StepOver) => {
                            debug.step_mode = crate::debug::StepMode::StepOver;
                            debug.step_frame_depth = self.frames.len();
                            break;
                        }
                        Ok(crate::debug::DebugCommand::StepOut) => {
                            debug.step_mode = crate::debug::StepMode::StepOut;
                            debug.step_frame_depth = self.frames.len();
                            break;
                        }
                        Ok(crate::debug::DebugCommand::Pause) => {}
                        Ok(crate::debug::DebugCommand::SetBreakpoints {
                            file,
                            lines,
                            reply,
                        }) => {
                            let ids = debug.set_breakpoints(&file, &lines);
                            let _ = reply.send(ids);
                        }
                        Ok(crate::debug::DebugCommand::GetStackTrace { reply }) => {
                            let _ = reply.send(self.debug_stack_trace());
                        }
                        Ok(crate::debug::DebugCommand::GetScopes {
                            frame_id,
                            reply,
                        }) => {
                            let _ = reply.send(self.debug_scopes(frame_id));
                        }
                        Ok(crate::debug::DebugCommand::GetVariables {
                            reference,
                            reply,
                        }) => {
                            let _ = reply.send(self.debug_variables(reference));
                        }
                        Ok(crate::debug::DebugCommand::Disconnect) => {
                            return Ok(Value::nil());
                        }
                        Err(_) => {
                            debug.step_mode = crate::debug::StepMode::Continue;
                            break;
                        }
                    }
                }
            }
        }
    }
}
```

This is essentially the same blocking command loop that was previously inside `run_inner`, but now it's outside. The VM's `run_inner` is now a pure cooperative stepper.

**Step 2: Add public `run_cooperative` method for WASM**

Add a method that exposes `run_inner` for external callers (like WASM bindings) that manage their own debug state:

```rust
/// Run the VM cooperatively: execute until completion or a debug stop.
/// The caller is responsible for managing the debug state between calls.
/// Returns `Stopped` when a breakpoint or step condition is hit.
/// Call again after updating `debug.step_mode` to resume.
pub fn run_cooperative(
    &mut self,
    ctx: &EvalContext,
    debug: &mut crate::debug::DebugState,
) -> Result<crate::debug::VmExecResult, SemaError> {
    self.run_inner(ctx, Some(debug))
}
```

**Step 3: Add `start_cooperative` that pushes initial frame + runs**

```rust
/// Start cooperative debug execution: push the initial frame and run
/// until the first stop or completion.
pub fn start_cooperative(
    &mut self,
    closure: Rc<Closure>,
    ctx: &EvalContext,
    debug: &mut crate::debug::DebugState,
) -> Result<crate::debug::VmExecResult, SemaError> {
    let base = self.stack.len();
    let n_locals = closure.func.chunk.n_locals as usize;
    self.stack.resize(base + n_locals, Value::nil());
    self.frames.push(CallFrame {
        closure,
        pc: 0,
        base,
        open_upvalues: None,
    });
    self.run_inner(ctx, Some(debug))
}
```

**Step 4: Update re-exports in `crates/sema-vm/src/lib.rs`**

The existing `pub use vm::VM` already covers the new methods.

**Step 5: Verify all VM tests pass**

Run: `cargo test -p sema-vm`
Expected: All tests pass. The native DAP path (execute_debug) preserves identical behavior.

**Step 6: Verify all integration tests pass**

Run: `cargo test -p sema`
Expected: All tests pass (including DAP integration tests).

**Step 7: Commit**

```bash
git add crates/sema-vm/src/vm.rs crates/sema-vm/src/debug.rs crates/sema-vm/src/lib.rs
git commit -m "refactor(vm): cooperative debug execution with VmExecResult

run_inner returns Stopped instead of blocking on channels.
execute_debug becomes a blocking driver loop around the cooperative core.
New start_cooperative/run_cooperative methods for WASM integration."
```

---

## Task 4: Add WASM debug API methods

**Files:**
- Modify: `crates/sema-wasm/Cargo.toml` (add sema-vm dependency)
- Modify: `crates/sema-wasm/src/lib.rs` (add debug session + API methods)

**Step 1: Add sema-vm dependency to sema-wasm**

Add to `crates/sema-wasm/Cargo.toml` under `[dependencies]`:
```toml
sema-vm = { path = "../sema-vm" }
```

**Step 2: Add debug session struct and thread-local**

Add near the top of `crates/sema-wasm/src/lib.rs` (after the existing thread_locals):

```rust
/// Active debug session state for cooperative VM execution.
struct DebugSession {
    vm: sema_vm::VM,
    debug: sema_vm::DebugState,
    closure: std::rc::Rc<sema_vm::Closure>,
    started: bool,
}
```

Add thread-local:
```rust
thread_local! {
    static DEBUG_SESSION: RefCell<Option<DebugSession>> = const { RefCell::new(None) };
}
```

**Step 3: Add `debugStart` method**

Add to `impl WasmInterpreter`:

```rust
/// Start a debug session. Compiles the code, sets breakpoints, and runs
/// until the first stop or completion.
/// `breakpoint_lines` is a JS array of 1-indexed line numbers.
/// Returns JSON: { "status": "stopped"|"finished"|"error", "line"?: number,
///   "reason"?: string, "value"?: string, "output"?: string[], "error"?: string }
#[wasm_bindgen(js_name = debugStart)]
pub fn debug_start(&self, code: &str, breakpoint_lines: &js_sys::Array) -> JsValue {
    OUTPUT.with(|o| o.borrow_mut().clear());
    LINE_BUF.with(|b| b.borrow_mut().clear());

    // Parse breakpoint lines from JS array
    let bp_lines: Vec<u32> = breakpoint_lines
        .iter()
        .filter_map(|v| v.as_f64().map(|n| n as u32))
        .collect();

    // Compile code using the Interpreter's compilation pipeline
    let (exprs, spans) = match sema_reader::read_many_with_spans(code) {
        Ok(r) => r,
        Err(e) => return self.debug_error_result(&e),
    };
    self.inner.ctx.merge_span_table(spans);
    if exprs.is_empty() {
        return self.debug_finished_result(Value::nil());
    }

    let mut expanded = Vec::new();
    for expr in &exprs {
        match self.inner.expand_for_vm(expr) {
            Ok(exp) => {
                if !exp.is_nil() {
                    expanded.push(exp);
                }
            }
            Err(e) => return self.debug_error_result(&e),
        }
    }

    if expanded.is_empty() {
        return self.debug_finished_result(Value::nil());
    }

    let (closure, functions) = match sema_vm::compile_program_with_spans(
        &expanded,
        &self.inner.ctx.span_map(),
    ) {
        Ok(r) => r,
        Err(e) => return self.debug_error_result(&e),
    };

    let mut vm = sema_vm::VM::new(self.inner.global_env.clone(), functions);
    let mut debug = sema_vm::DebugState::new_headless();

    // Set breakpoints — use a synthetic file path since playground code has no file
    if !bp_lines.is_empty() {
        let file = std::path::PathBuf::from("<playground>");
        debug.set_breakpoints(&file, &bp_lines);
    }

    // Step into the first line (stop on entry)
    debug.step_mode = sema_vm::StepMode::StepInto;

    match vm.start_cooperative(closure.clone(), &self.inner.ctx, &mut debug) {
        Ok(sema_vm::VmExecResult::Stopped(info)) => {
            DEBUG_SESSION.with(|s| {
                *s.borrow_mut() = Some(DebugSession {
                    vm,
                    debug,
                    closure,
                    started: true,
                });
            });
            self.debug_stopped_result(&info)
        }
        Ok(sema_vm::VmExecResult::Finished(v)) => {
            self.debug_finished_result(v)
        }
        Err(e) => self.debug_error_result(&e),
    }
}
```

Note: `expand_for_vm` is a private method on `Interpreter`. We'll need to either:
- Make it `pub` in `sema-eval/src/eval.rs`
- Or duplicate the logic (parse + compile directly without macro expansion)

The simplest fix: make `expand_for_vm` public. Add `pub` to `fn expand_for_vm` in `crates/sema-eval/src/eval.rs:180`.

Also note: for breakpoints to match, compiled functions need `source_file` set. The playground has no file — we use `compile_program_with_spans` which doesn't set `source_file`. We need to use `compile_program_with_spans_and_source` with a synthetic path like `PathBuf::from("<playground>")`.

**Step 4: Add resume methods**

```rust
/// Resume debug execution with Continue mode.
#[wasm_bindgen(js_name = debugContinue)]
pub fn debug_continue(&self) -> JsValue {
    self.debug_resume(sema_vm::StepMode::Continue)
}

/// Resume debug execution with StepInto mode.
#[wasm_bindgen(js_name = debugStepInto)]
pub fn debug_step_into(&self) -> JsValue {
    self.debug_resume(sema_vm::StepMode::StepInto)
}

/// Resume debug execution with StepOver mode.
#[wasm_bindgen(js_name = debugStepOver)]
pub fn debug_step_over(&self) -> JsValue {
    self.debug_resume(sema_vm::StepMode::StepOver)
}

/// Resume debug execution with StepOut mode.
#[wasm_bindgen(js_name = debugStepOut)]
pub fn debug_step_out(&self) -> JsValue {
    self.debug_resume(sema_vm::StepMode::StepOut)
}

fn debug_resume(&self, mode: sema_vm::StepMode) -> JsValue {
    DEBUG_SESSION.with(|s| {
        let mut session = s.borrow_mut();
        let Some(ref mut sess) = *session else {
            return self.debug_error_str("No active debug session");
        };

        sess.debug.step_mode = mode;
        if mode != sema_vm::StepMode::Continue {
            sess.debug.step_frame_depth = sess.vm.frame_count();
        }

        match sess.vm.run_cooperative(&self.inner.ctx, &mut sess.debug) {
            Ok(sema_vm::VmExecResult::Stopped(info)) => {
                self.debug_stopped_result(&info)
            }
            Ok(sema_vm::VmExecResult::Finished(v)) => {
                let result = self.debug_finished_result(v);
                *session = None; // Clean up session
                result
            }
            Err(e) => {
                let result = self.debug_error_result(&e);
                *session = None;
                result
            }
        }
    })
}
```

Note: we need `VM::frame_count()` — a simple public getter: `pub fn frame_count(&self) -> usize { self.frames.len() }`. Add this to `vm.rs`.

**Step 5: Add `debugStop` method**

```rust
/// Stop the current debug session and discard state.
#[wasm_bindgen(js_name = debugStop)]
pub fn debug_stop(&self) {
    DEBUG_SESSION.with(|s| {
        *s.borrow_mut() = None;
    });
}
```

**Step 6: Add `debugGetLocals` method**

```rust
/// Get local variables of the current (top) frame. Returns JSON array.
#[wasm_bindgen(js_name = debugGetLocals)]
pub fn debug_get_locals(&self) -> JsValue {
    DEBUG_SESSION.with(|s| {
        let session = s.borrow();
        let Some(ref sess) = *session else {
            return js_sys::Array::new().into();
        };
        let locals = sess.vm.debug_locals(sess.vm.frame_count().saturating_sub(1));
        let arr = js_sys::Array::new();
        for var in &locals {
            let obj = js_sys::Object::new();
            js_sys::Reflect::set(&obj, &"name".into(), &JsValue::from_str(&var.name)).unwrap();
            js_sys::Reflect::set(&obj, &"value".into(), &JsValue::from_str(&var.value)).unwrap();
            js_sys::Reflect::set(&obj, &"type".into(), &JsValue::from_str(&var.type_name)).unwrap();
            arr.push(&obj);
        }
        arr.into()
    })
}
```

**Step 7: Add `debugGetStackTrace` method**

```rust
/// Get the current call stack. Returns JSON array.
#[wasm_bindgen(js_name = debugGetStackTrace)]
pub fn debug_get_stack_trace(&self) -> JsValue {
    DEBUG_SESSION.with(|s| {
        let session = s.borrow();
        let Some(ref sess) = *session else {
            return js_sys::Array::new().into();
        };
        let frames = sess.vm.debug_stack_trace();
        let arr = js_sys::Array::new();
        for frame in &frames {
            let obj = js_sys::Object::new();
            js_sys::Reflect::set(&obj, &"name".into(), &JsValue::from_str(&frame.name)).unwrap();
            js_sys::Reflect::set(&obj, &"line".into(), &JsValue::from_f64(frame.line as f64)).unwrap();
            js_sys::Reflect::set(&obj, &"column".into(), &JsValue::from_f64(frame.column as f64)).unwrap();
            arr.push(&obj);
        }
        arr.into()
    })
}
```

**Step 8: Add `debugSetBreakpoints` method**

```rust
/// Update breakpoints during a debug session.
#[wasm_bindgen(js_name = debugSetBreakpoints)]
pub fn debug_set_breakpoints(&self, lines: &js_sys::Array) {
    let bp_lines: Vec<u32> = lines
        .iter()
        .filter_map(|v| v.as_f64().map(|n| n as u32))
        .collect();
    DEBUG_SESSION.with(|s| {
        if let Some(ref mut sess) = *s.borrow_mut() {
            let file = std::path::PathBuf::from("<playground>");
            sess.debug.set_breakpoints(&file, &bp_lines);
        }
    });
}
```

**Step 9: Add `debugIsActive` check**

```rust
/// Check if a debug session is currently active.
#[wasm_bindgen(js_name = debugIsActive)]
pub fn debug_is_active(&self) -> bool {
    DEBUG_SESSION.with(|s| s.borrow().is_some())
}
```

**Step 10: Add helper methods for JSON result formatting**

```rust
fn debug_stopped_result(&self, info: &sema_vm::StopInfo) -> JsValue {
    let output = take_output();
    let output_json = output
        .iter()
        .map(|s| format!("\"{}\"", escape_json(s)))
        .collect::<Vec<_>>()
        .join(",");
    let reason = match info.reason {
        sema_vm::StopReason::Breakpoint => "breakpoint",
        sema_vm::StopReason::Step => "step",
        sema_vm::StopReason::Pause => "pause",
        sema_vm::StopReason::Entry => "entry",
    };
    let json_str = format!(
        "{{\"status\":\"stopped\",\"line\":{},\"reason\":\"{}\",\"output\":[{}]}}",
        info.line, reason, output_json,
    );
    js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
}

fn debug_finished_result(&self, val: Value) -> JsValue {
    let output = take_output();
    let val_str = if val.is_nil() {
        "null".to_string()
    } else {
        format!("\"{}\"", escape_json(&sema_core::pretty_print(&val, 80)))
    };
    let output_json = output
        .iter()
        .map(|s| format!("\"{}\"", escape_json(s)))
        .collect::<Vec<_>>()
        .join(",");
    let json_str = format!(
        "{{\"status\":\"finished\",\"value\":{},\"output\":[{}],\"error\":null}}",
        val_str, output_json,
    );
    js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
}

fn debug_error_result(&self, e: &sema_core::SemaError) -> JsValue {
    let output = take_output();
    let mut err_str = format!("{}", e.inner());
    if let Some(trace) = e.stack_trace() {
        err_str.push_str(&format!("\n{trace}"));
    }
    if let Some(hint) = e.hint() {
        err_str.push_str(&format!("\n  hint: {hint}"));
    }
    let output_json = output
        .iter()
        .map(|s| format!("\"{}\"", escape_json(s)))
        .collect::<Vec<_>>()
        .join(",");
    let json_str = format!(
        "{{\"status\":\"error\",\"output\":[{}],\"error\":\"{}\"}}",
        output_json,
        escape_json(&err_str),
    );
    js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
}

fn debug_error_str(&self, msg: &str) -> JsValue {
    let json_str = format!(
        "{{\"status\":\"error\",\"output\":[],\"error\":\"{}\"}}",
        escape_json(msg),
    );
    js_sys::JSON::parse(&json_str).unwrap_or(JsValue::NULL)
}
```

**Step 11: Make `expand_for_vm` public**

In `crates/sema-eval/src/eval.rs:180`, change `fn expand_for_vm` to `pub fn expand_for_vm`.

**Step 12: Add `frame_count()` to VM**

In `crates/sema-vm/src/vm.rs`, add:
```rust
/// Number of active call frames (for setting step_frame_depth).
pub fn frame_count(&self) -> usize {
    self.frames.len()
}
```

**Step 13: Verify WASM compilation**

Run: `cargo build -p sema-wasm --target wasm32-unknown-unknown`
Expected: Compiles successfully. (Can't run WASM tests from CLI, but compile check is sufficient.)

**Step 14: Verify all Rust tests still pass**

Run: `cargo test -p sema-vm && cargo test -p sema`
Expected: All pass.

**Step 15: Commit**

```bash
git add crates/sema-wasm/ crates/sema-eval/src/eval.rs crates/sema-vm/src/vm.rs
git commit -m "feat(wasm): add cooperative debug API for playground debugger

New WASM methods: debugStart, debugContinue, debugStepInto,
debugStepOver, debugStepOut, debugStop, debugGetLocals,
debugGetStackTrace, debugSetBreakpoints, debugIsActive."
```

---

## Task 5: Add line number gutter to the editor

**Files:**
- Modify: `playground/index.html`
- Modify: `playground/style.css`
- Modify: `playground/src/app.js`

**Step 1: Add gutter element to HTML**

In `playground/index.html`, inside `<div class="editor-wrap">`, add the gutter div before the textarea:

```html
<div class="editor-gutter" id="editor-gutter"></div>
```

**Step 2: Add gutter CSS**

Add to `playground/style.css`, after the `.editor-wrap` styles:

```css
/* ── Line number gutter ── */
.editor-gutter {
    position: absolute;
    top: 0;
    left: 0;
    width: 40px;
    height: 100%;
    padding-top: 1.25rem;
    font-family: var(--mono);
    font-size: 13px;
    line-height: 1.65;
    color: var(--text-dim);
    background: var(--bg-editor);
    border-right: 1px solid var(--border);
    overflow: hidden;
    z-index: 2;
    user-select: none;
    cursor: default;
}
.gutter-line {
    padding: 0 6px 0 0;
    text-align: right;
    height: 1.65em;
    position: relative;
    cursor: pointer;
}
.gutter-line:hover {
    color: var(--text);
}
.gutter-line.breakpoint::before {
    content: '●';
    position: absolute;
    left: 4px;
    color: var(--error);
    font-size: 10px;
    line-height: 1.65em;
}
.gutter-line.breakpoint {
    color: var(--error);
}
.gutter-line.current-line {
    background: rgba(200, 168, 85, 0.15);
    color: var(--gold);
}
```

**Step 3: Adjust editor padding for gutter**

Change the shared `textarea#editor` and `.editor-highlight` padding from `1.25rem` to `1.25rem 1.25rem 1.25rem calc(40px + 1rem)`:

```css
.editor-highlight,
textarea#editor {
    /* ... existing properties ... */
    padding: 1.25rem 1.25rem 1.25rem calc(40px + 1rem);
}
```

**Step 4: Add gutter rendering in app.js**

Add to `app.js` after the `syncScroll` function:

```javascript
// ── Line number gutter ──

const gutterEl = document.getElementById('editor-gutter');
let breakpoints = new Set();
let currentDebugLine = null;

function updateGutter() {
    const code = editorEl.value;
    const lineCount = (code.match(/\n/g) || []).length + 1;
    gutterEl.innerHTML = '';
    for (let i = 1; i <= lineCount; i++) {
        const line = document.createElement('div');
        line.className = 'gutter-line';
        if (breakpoints.has(i)) line.classList.add('breakpoint');
        if (currentDebugLine === i) line.classList.add('current-line');
        line.textContent = i;
        line.addEventListener('click', () => toggleBreakpoint(i));
        gutterEl.appendChild(line);
    }
}

function toggleBreakpoint(line) {
    if (breakpoints.has(line)) {
        breakpoints.delete(line);
    } else {
        breakpoints.add(line);
    }
    updateGutter();
    // Update breakpoints in active debug session
    if (interp && interp.debugIsActive()) {
        interp.debugSetBreakpoints(Array.from(breakpoints));
    }
}
```

**Step 5: Sync gutter scroll with editor**

Update `syncScroll`:
```javascript
function syncScroll() {
    hlEl.scrollTop = editorEl.scrollTop;
    hlEl.scrollLeft = editorEl.scrollLeft;
    gutterEl.scrollTop = editorEl.scrollTop;
}
```

**Step 6: Hook gutter update to editor changes**

In `scheduleHighlight`, add `updateGutter()`:
```javascript
function scheduleHighlight() {
    cancelAnimationFrame(hlRaf);
    hlRaf = requestAnimationFrame(() => {
        hlEl.innerHTML = highlightSema(editorEl.value);
        updateGutter();
    });
}
```

**Step 7: Verify visually**

Build the WASM module and open the playground:
```bash
cd playground && node build.mjs
```
Open `playground/index.html` in a browser. Verify:
- Line numbers appear in a gutter on the left
- Gutter scrolls with the editor
- Clicking a line number toggles a red dot (breakpoint marker)

**Step 8: Commit**

```bash
git add playground/index.html playground/style.css playground/src/app.js
git commit -m "feat(playground): add line number gutter with breakpoint toggling"
```

---

## Task 6: Add debug controls and state machine

**Files:**
- Modify: `playground/index.html`
- Modify: `playground/style.css`
- Modify: `playground/src/app.js`

**Step 1: Add debug button to HTML**

In `playground/index.html`, after the Run button, add the debug button:

```html
<button class="run-btn debug-btn" id="debug-btn" data-testid="debug-btn" disabled data-tooltip="Debug with breakpoints">Debug</button>
```

And add the debug control toolbar (hidden by default):

```html
<div class="debug-controls hidden" id="debug-controls" data-testid="debug-controls">
    <button class="debug-ctrl-btn" id="dbg-continue" data-tooltip="Continue (F5)">▶</button>
    <button class="debug-ctrl-btn" id="dbg-step-over" data-tooltip="Step Over (F10)">⏭</button>
    <button class="debug-ctrl-btn" id="dbg-step-into" data-tooltip="Step Into (F11)">↓</button>
    <button class="debug-ctrl-btn" id="dbg-step-out" data-tooltip="Step Out (Shift+F11)">↑</button>
    <button class="debug-ctrl-btn dbg-stop" id="dbg-stop" data-tooltip="Stop debugging">⬛</button>
</div>
```

Place the debug-controls div inside the `.pane-header` of the Source pane, next to the existing controls.

**Step 2: Add debug control CSS**

```css
/* ── Debug controls ── */
.debug-btn {
    background: transparent;
    color: var(--gold);
    border: 1px solid var(--gold-dim);
}
.debug-btn:hover {
    background: var(--gold-glow);
}
.debug-controls {
    display: flex;
    align-items: center;
    gap: 0.25rem;
}
.debug-controls.hidden { display: none; }
.debug-ctrl-btn {
    font-family: system-ui, -apple-system, sans-serif;
    font-size: 0.8rem;
    width: 28px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: transparent;
    color: var(--text);
    cursor: pointer;
    transition: background 0.15s, color 0.15s, border-color 0.15s;
}
.debug-ctrl-btn:hover {
    background: var(--gold-glow);
    color: var(--gold);
    border-color: var(--gold-dim);
}
.debug-ctrl-btn.dbg-stop:hover {
    color: var(--error);
    border-color: var(--error);
}
```

**Step 3: Add debug state machine in app.js**

```javascript
// ── Debug state machine ──

let debugState = 'idle'; // 'idle' | 'running' | 'paused'

const debugBtn = document.getElementById('debug-btn');
const debugControls = document.getElementById('debug-controls');

function setDebugState(state) {
    debugState = state;
    const runBtn = document.getElementById('run-btn');
    const fmtBtn = document.getElementById('fmt-btn');

    switch (state) {
        case 'idle':
            debugBtn.disabled = false;
            debugBtn.classList.remove('hidden');
            runBtn.disabled = false;
            fmtBtn.disabled = false;
            debugControls.classList.add('hidden');
            editorEl.readOnly = false;
            currentDebugLine = null;
            updateGutter();
            document.getElementById('status').textContent = 'Ready';
            document.getElementById('status').className = 'status-text status-ready';
            break;
        case 'running':
            debugBtn.disabled = true;
            runBtn.disabled = true;
            fmtBtn.disabled = true;
            debugControls.classList.remove('hidden');
            editorEl.readOnly = true;
            document.getElementById('status').textContent = 'Debugging…';
            document.getElementById('status').className = 'status-text status-loading';
            break;
        case 'paused':
            debugBtn.disabled = true;
            runBtn.disabled = true;
            fmtBtn.disabled = true;
            debugControls.classList.remove('hidden');
            editorEl.readOnly = true;
            document.getElementById('status').textContent = `Paused at line ${currentDebugLine}`;
            document.getElementById('status').className = 'status-text status-loading';
            break;
    }
}
```

**Step 4: Add debug start/resume handlers**

```javascript
function handleDebugResult(result) {
    // Append any output produced during this step
    if (result.output && result.output.length > 0) {
        for (const line of result.output) {
            const div = document.createElement('div');
            div.className = 'output-line';
            div.textContent = line;
            outputEl.appendChild(div);
        }
    }

    if (result.status === 'stopped') {
        currentDebugLine = result.line;
        updateGutter();
        scrollToLine(result.line);
        updateVariablesPanel();
        setDebugState('paused');
    } else if (result.status === 'finished') {
        if (result.value !== null) {
            const div = document.createElement('div');
            div.className = 'output-value';
            div.textContent = `=> ${result.value}`;
            outputEl.appendChild(div);
        }
        interp.debugStop();
        setDebugState('idle');
    } else if (result.status === 'error') {
        const div = document.createElement('div');
        div.className = 'output-error';
        div.textContent = result.error;
        outputEl.appendChild(div);
        interp.debugStop();
        setDebugState('idle');
    }
}

function scrollToLine(line) {
    // Scroll the editor to make the given line visible
    const lineHeight = parseFloat(getComputedStyle(editorEl).lineHeight);
    const targetScroll = (line - 1) * lineHeight - editorEl.clientHeight / 2 + lineHeight;
    editorEl.scrollTop = Math.max(0, targetScroll);
    syncScroll();
}

debugBtn.addEventListener('click', () => {
    if (!interp || debugState !== 'idle') return;
    const code = editorEl.value;
    if (!code.trim()) return;

    outputEl.innerHTML = '';
    setDebugState('running');

    const result = interp.debugStart(code, Array.from(breakpoints));
    handleDebugResult(result);
});

document.getElementById('dbg-continue').addEventListener('click', () => {
    if (!interp || debugState !== 'paused') return;
    setDebugState('running');
    const result = interp.debugContinue();
    handleDebugResult(result);
});

document.getElementById('dbg-step-over').addEventListener('click', () => {
    if (!interp || debugState !== 'paused') return;
    setDebugState('running');
    const result = interp.debugStepOver();
    handleDebugResult(result);
});

document.getElementById('dbg-step-into').addEventListener('click', () => {
    if (!interp || debugState !== 'paused') return;
    setDebugState('running');
    const result = interp.debugStepInto();
    handleDebugResult(result);
});

document.getElementById('dbg-step-out').addEventListener('click', () => {
    if (!interp || debugState !== 'paused') return;
    setDebugState('running');
    const result = interp.debugStepOut();
    handleDebugResult(result);
});

document.getElementById('dbg-stop').addEventListener('click', () => {
    if (!interp) return;
    interp.debugStop();
    setDebugState('idle');
});
```

**Step 5: Add keyboard shortcuts for debug**

Add to the existing `keydown` handler on `editorEl`:

```javascript
// Debug keyboard shortcuts
if (e.key === 'F5' && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-continue').click();
}
if (e.key === 'F10' && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-step-over').click();
}
if (e.key === 'F11' && !e.shiftKey && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-step-into').click();
}
if (e.key === 'F11' && e.shiftKey && debugState === 'paused') {
    e.preventDefault();
    document.getElementById('dbg-step-out').click();
}
if (e.key === 'Escape' && debugState !== 'idle') {
    e.preventDefault();
    document.getElementById('dbg-stop').click();
}
```

**Step 6: Enable debug button when interpreter is ready**

In the `main()` function, after `document.getElementById('fmt-btn').disabled = false;`, add:
```javascript
document.getElementById('debug-btn').disabled = false;
```

**Step 7: Commit**

```bash
git add playground/
git commit -m "feat(playground): add debug controls and state machine"
```

---

## Task 7: Add current line highlighting and variables panel

**Files:**
- Modify: `playground/style.css`
- Modify: `playground/src/app.js`
- Modify: `playground/index.html`

**Step 1: Add current line highlight overlay CSS**

```css
/* ── Debug current line highlight ── */
.debug-line-highlight {
    position: absolute;
    left: 40px;
    right: 0;
    height: 1.65em;
    background: rgba(200, 168, 85, 0.1);
    border-left: 2px solid var(--gold);
    pointer-events: none;
    z-index: 0;
}
```

**Step 2: Add highlight element rendering**

In `updateGutter()`, after rendering all gutter lines, add the line highlight to `.editor-wrap`:

```javascript
// Update current line highlight
const existingHL = document.querySelector('.debug-line-highlight');
if (existingHL) existingHL.remove();

if (currentDebugLine !== null) {
    const lineHeight = parseFloat(getComputedStyle(editorEl).lineHeight) || 21.45;
    const paddingTop = parseFloat(getComputedStyle(editorEl).paddingTop) || 20;
    const hl = document.createElement('div');
    hl.className = 'debug-line-highlight';
    hl.style.top = `${paddingTop + (currentDebugLine - 1) * lineHeight - editorEl.scrollTop}px`;
    editorEl.parentElement.appendChild(hl);
}
```

Also update the highlight position in `syncScroll`.

**Step 3: Add variables panel in the output area**

When paused, show a variables section above the output:

```javascript
function updateVariablesPanel() {
    // Remove existing variables panel
    const existing = document.getElementById('debug-vars');
    if (existing) existing.remove();

    if (debugState !== 'paused' || !interp) return;

    const locals = interp.debugGetLocals();
    if (!locals || locals.length === 0) return;

    const panel = document.createElement('div');
    panel.id = 'debug-vars';
    panel.className = 'debug-vars-panel';

    const header = document.createElement('div');
    header.className = 'debug-vars-header';
    header.textContent = 'Variables';
    panel.appendChild(header);

    for (const v of locals) {
        const row = document.createElement('div');
        row.className = 'debug-var-row';
        const name = document.createElement('span');
        name.className = 'debug-var-name';
        name.textContent = v.name;
        const eq = document.createTextNode(' = ');
        const val = document.createElement('span');
        val.className = 'debug-var-value';
        val.textContent = v.value;
        const type = document.createElement('span');
        type.className = 'debug-var-type';
        type.textContent = ` (${v.type})`;
        row.appendChild(name);
        row.appendChild(eq);
        row.appendChild(val);
        row.appendChild(type);
        panel.appendChild(row);
    }

    outputEl.insertBefore(panel, outputEl.firstChild);
}
```

**Step 4: Add variables panel CSS**

```css
/* ── Debug variables panel ── */
.debug-vars-panel {
    border-bottom: 1px solid var(--border);
    padding: 0.75rem 1.25rem;
    margin-bottom: 0.5rem;
}
.debug-vars-header {
    font-size: 0.65rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-dim);
    margin-bottom: 0.5rem;
}
.debug-var-row {
    font-family: var(--mono);
    font-size: 13px;
    line-height: 1.65;
    padding: 0.1rem 0;
}
.debug-var-name {
    color: var(--gold);
}
.debug-var-value {
    color: var(--text-bright);
}
.debug-var-type {
    color: var(--text-dim);
    font-size: 0.7rem;
}
```

**Step 5: Clean up variables panel on debug stop**

In the `setDebugState('idle')` handler, add:
```javascript
const varsPanel = document.getElementById('debug-vars');
if (varsPanel) varsPanel.remove();
```

**Step 6: Commit**

```bash
git add playground/
git commit -m "feat(playground): add current line highlight and variables panel"
```

---

## Task 8: Build WASM and end-to-end test

**Files:**
- Build scripts and manual testing

**Step 1: Build the WASM module**

```bash
cd playground && node build.mjs
```

Expected: Build succeeds, generates `playground/pkg/` with updated WASM module and JS bindings.

If build.mjs doesn't handle the build, use wasm-pack directly:
```bash
wasm-pack build crates/sema-wasm --target web --out-dir ../../playground/pkg
```

**Step 2: Test non-debug functionality**

Open the playground in a browser. Verify:
- Run button still works (tree-walker and VM)
- Syntax highlighting works
- VFS panel works
- Examples load correctly

**Step 3: Test debug flow**

1. Enter code:
```lisp
(define x 10)
(define y 20)
(define z (+ x y))
(println z)
```
2. Click line 3 in the gutter to set a breakpoint (red dot appears)
3. Click "Debug" button
4. Verify: VM stops at entry (line 1), gutter shows golden highlight on line 1
5. Click "Continue" (▶)
6. Verify: VM stops at line 3 (breakpoint), variables panel shows `x = 10`, `y = 20`
7. Click "Step Over" (⏭)
8. Verify: VM advances to line 4, variables panel shows `z = 30`
9. Click "Continue"
10. Verify: Output shows "30", debug session ends, UI returns to idle

**Step 4: Test step into/out with functions**

```lisp
(define (add a b) (+ a b))
(define result (add 3 4))
(println result)
```
1. Set breakpoint on line 2
2. Debug → Continue (stops at line 2)
3. Step Into → should enter the `add` function (line 1)
4. Variables panel shows `a = 3`, `b = 4`
5. Step Out → returns to line 2
6. Continue → finishes, output shows "7"

**Step 5: Test stop button**

1. Start debug on any code
2. Click Stop (⬛)
3. Verify: debug session ends cleanly, UI returns to idle

**Step 6: Commit all final adjustments**

```bash
git add -A  # Only if all changes are playground-related
git commit -m "feat(playground): complete debugger UI with breakpoints, stepping, and variables

Adds cooperative VM execution (VmExecResult::Stopped), WASM debug API
(debugStart/Continue/StepInto/StepOver/StepOut/Stop/GetLocals/GetStackTrace),
and playground UI (line gutter, breakpoint markers, debug controls, current
line highlight, variables panel)."
```

---

## Summary of all modified files

### Rust (sema-vm)
- `crates/sema-vm/src/debug.rs` — Add VmExecResult, StopInfo, resume_skip, new_headless()
- `crates/sema-vm/src/vm.rs` — Cooperative run_inner, refactored execute_debug, new run_cooperative/start_cooperative/frame_count
- `crates/sema-vm/src/lib.rs` — Updated re-exports

### Rust (sema-eval)
- `crates/sema-eval/src/eval.rs` — Make expand_for_vm public

### Rust (sema-wasm)
- `crates/sema-wasm/Cargo.toml` — Add sema-vm dependency
- `crates/sema-wasm/src/lib.rs` — DebugSession, 10 new WASM API methods, result helpers

### Playground
- `playground/index.html` — Gutter div, debug button, debug controls
- `playground/style.css` — Gutter, breakpoint, debug controls, line highlight, variables panel styles
- `playground/src/app.js` — Gutter rendering, breakpoint toggling, debug state machine, debug handlers, keyboard shortcuts, variables panel

### Total: ~10 files, ~500 lines of new code
