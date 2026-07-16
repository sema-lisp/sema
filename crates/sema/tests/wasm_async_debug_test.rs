//! Gate (Slice 2): breakpoints INSIDE async tasks STOP + CONTINUE in the
//! COOPERATIVE (WASM playground) debugger — now driven on the UNIFIED RUNTIME's
//! cooperative debug path (`Runtime::drive` surfacing `DriveState::DebugStopped`,
//! `Runtime::debug_resume`, `Runtime::with_paused_task_vm`), NOT the retired
//! legacy `VM::start_cooperative` / `run_cooperative` scheduler.
//!
//! This replicates the cooperative WASM flow WITHOUT a browser, mirroring
//! `SemaPlayground::debug_start`/`debugPoll` in `crates/sema-wasm/src/lib.rs`:
//! read_many_with_spans → compile_program_with_spans → DebugState::new_headless →
//! set_valid_breakpoint_lines + set_breakpoints → submit the seeded VM as a root
//! on the interpreter's persistent runtime → drive bounded turns under an
//! `ActiveDebugGuard`, mapping `DriveState::DebugStopped` to `Stopped`.
//!
//! Why the native gate (`dap_async_breakpoint_test.rs`) is not enough: the
//! cooperative path is step-driven and must NOT block on a command channel — a
//! headless `DebugState` makes `run_quantum_debug` RETURN `Stopped(info)` out of
//! the quantum, which the runtime turns into a `DebugStopped` barrier that a
//! later `debug_resume` clears. Slice 1 fixed only the blocking native path.
//!
//! Contract under test: the first drive returns a stop whose info line == the
//! async breakpoint line (control: a SYNC-line breakpoint already does this — it
//! proves the harness). A follow-up resume (simulating Continue) eventually
//! settles the root (`Finished`).

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use sema_core::runtime::TaskOutcome;
use sema_eval::Interpreter;
use sema_vm::runtime::{DriveBudget, DriveState, RootHandle, RootPoll};
use sema_vm::{ActiveDebugGuard, DebugState, StepMode, VmExecResult, VM};

/// Compile `source` cooperatively (spans → `<playground>`, matching the WASM
/// path), build a seeded VM plus a headless `DebugState` with `bp_lines` set, and
/// return them ready to submit as a runtime root.
fn build(interpreter: &Interpreter, source: &str, bp_lines: &[u32]) -> (VM, DebugState) {
    let (vals, span_map) = sema_reader::read_many_with_spans(source).expect("parses");
    // The WASM debugger attributes spans to this synthetic path; canonicalize
    // fails for it everywhere, so both the compiler and set_breakpoints keep it
    // verbatim and agree on the breakpoint key.
    let source_file = PathBuf::from("<playground>");
    let prog = sema_vm::compile_program_with_spans(&vals, &span_map, Some(source_file.clone()))
        .expect("compiles");

    let valid = sema_vm::valid_breakpoint_lines(&prog.closure, &prog.functions);
    let snapped: Vec<u32> = bp_lines
        .iter()
        .map(|&l| {
            let s = sema_vm::snap_breakpoint_line(l, &valid).expect("bp line snaps to executable");
            assert_eq!(
                s, l,
                "test programs use directly-executable breakpoint lines"
            );
            s
        })
        .collect();

    let mut debug = DebugState::new_headless();
    debug.set_valid_breakpoint_lines(sema_vm::valid_breakpoint_lines_by_file(
        &prog.closure,
        &prog.functions,
    ));
    let resolved = debug.set_breakpoints(&source_file, &snapped);
    assert!(
        resolved.iter().any(|bp| bp.verified),
        "breakpoints {snapped:?} did not resolve: {resolved:?}"
    );
    debug.step_mode = StepMode::Continue;
    debug.instructions_remaining = 5_000_000;

    let mut vm = VM::new(
        interpreter.global_env.clone(),
        prog.functions,
        &[],
        prog.main_cache_slots,
    )
    .expect("VM builds");
    vm.seed_main_frame(prog.closure);
    (vm, debug)
}

/// Drive the runtime under `debug` until it hits a cooperative debug stop or the
/// root settles. Mirrors `SemaPlayground::debugPoll`'s bounded drive loop: each
/// turn runs under an `ActiveDebugGuard` so `run_parked_quantum` runs the
/// debug-aware quantum; a `DriveState::DebugStopped` becomes `Stopped(info)`.
fn drive_debug(interp: &Interpreter, debug: &mut DebugState, handle: &RootHandle) -> VmExecResult {
    let runtime = interp.runtime();
    let budget = DriveBudget::host_default();
    for _ in 0..200_000 {
        match handle.poll_result() {
            RootPoll::Ready(settlement) => {
                return match &settlement.outcome {
                    TaskOutcome::Returned(v) => VmExecResult::Finished(v.clone()),
                    TaskOutcome::Failed(e) => panic!("debug root failed: {e}"),
                    TaskOutcome::Cancelled(r) => panic!("debug root cancelled: {r:?}"),
                };
            }
            RootPoll::Pending => {}
            RootPoll::Aborted(fault) => panic!("debug root aborted: {fault:?}"),
            RootPoll::RuntimeDropped | RootPoll::InvariantViolation => {
                panic!("debug runtime invariant violation")
            }
        }
        debug.instructions_remaining = 5_000_000;
        let state = {
            let _guard = ActiveDebugGuard::enter(debug);
            runtime.drive(&budget).expect("drive does not fault")
        };
        match state {
            DriveState::DebugStopped { info, .. } => return VmExecResult::Stopped(info),
            DriveState::Progress { .. } | DriveState::Quiescent | DriveState::ShutdownComplete => {}
            DriveState::Idle { .. } => {
                // These pure-compute test programs never park on a timer or an
                // external inbox; an Idle turn without a settled root would be a
                // genuine deadlock.
                panic!("debug drive went Idle without settling the root: {state:?}");
            }
        }
    }
    panic!("debug drive did not reach a stop or settlement");
}

/// One cooperative debug session bound to a persistent interpreter runtime.
struct Coop {
    interpreter: Interpreter,
    debug: DebugState,
    handle: RootHandle,
}

impl Coop {
    /// Build + submit `source` with a single breakpoint on `bp_line`, drive to
    /// the first stop/finish, and return the session plus that first result.
    fn start(source: &str, bp_line: u32) -> (Self, VmExecResult) {
        let interpreter = Interpreter::new();
        let (vm, mut debug) = build(&interpreter, source, &[bp_line]);
        let handle = interpreter.runtime().submit_root(vm).expect("root submits");
        let first = drive_debug(&interpreter, &mut debug, &handle);
        (
            Coop {
                interpreter,
                debug,
                handle,
            },
            first,
        )
    }

    /// Apply a step/continue command at the current stop and drive one slice.
    /// Mirrors `WasmInterpreter::debug_resume`: at an async stop the step depth is
    /// measured against the PAUSED TASK's VM (the one that resumes), not any main
    /// VM parked at the await.
    fn resume(&mut self, mode: StepMode) -> VmExecResult {
        self.debug.step_mode = mode;
        if mode != StepMode::Continue {
            if let Some(depth) = self
                .interpreter
                .runtime()
                .with_paused_task_vm(|tvm| tvm.frame_count())
            {
                self.debug.step_frame_depth = depth;
            }
        }
        self.interpreter.runtime().debug_resume();
        drive_debug(&self.interpreter, &mut self.debug, &self.handle)
    }

    fn step(&mut self, mode: StepMode) -> VmExecResult {
        self.resume(mode)
    }

    /// Simulate a Continue / poll loop: resume past each stop with Continue until
    /// the root settles. Panics if it does not finish within the tick budget.
    fn continue_to_finish(&mut self) {
        for _ in 0..10_000 {
            if self.interpreter.runtime().is_debug_paused() {
                self.debug.step_mode = StepMode::Continue;
                self.interpreter.runtime().debug_resume();
            }
            match drive_debug(&self.interpreter, &mut self.debug, &self.handle) {
                VmExecResult::Finished(_) => return,
                VmExecResult::Stopped(_) => continue,
                other => panic!("unexpected cooperative drive result: {other:?}"),
            }
        }
        panic!("program did not finish within the tick budget");
    }
}

fn assert_stopped_at(result: &VmExecResult, line: u32) {
    match result {
        VmExecResult::Stopped(info) => assert_eq!(
            info.line, line,
            "expected cooperative Stopped on line {line}, got {info:?}"
        ),
        other => panic!("expected Stopped on line {line}, got {other:?}"),
    }
}

/// CONTROL: sync breakpoint stops + continues cooperatively. Proves the harness
/// is valid (so a failure of the async case below is a real gap, not a broken
/// test).
#[test]
fn coop_sync_breakpoint_full_cycle() {
    let (mut coop, first) = Coop::start("(define x 1)\n(define y (+ x 2))\n(+ x y)\n", 2);
    assert_stopped_at(&first, 2);
    coop.continue_to_finish();
}

/// THE GATE: a breakpoint on a line that runs only INSIDE an async task must
/// surface cooperatively as `Stopped` (line == the async breakpoint line), and a
/// follow-up resume (Continue) must drive it to `Finished`.
#[test]
fn coop_async_task_breakpoint_stops_and_continues() {
    // Line 2 is `(+ 1 2)` — executes only inside the spawned task body.
    let (mut coop, first) = Coop::start(
        "(define p (async/spawn (fn ()\n  (+ 1 2))))\n(await p)\n",
        2,
    );
    match &first {
        VmExecResult::Stopped(info) => assert_eq!(
            info.line, 2,
            "async-task breakpoint should stop on line 2 (inside the thunk), got {info:?} \
             (a swallowed stop means Slice 2 regressed)"
        ),
        other => panic!("expected Stopped inside the async task, got {other:?}"),
    }
    coop.continue_to_finish();
}

/// THE GATE (deeper / multi-task): two spawned tasks plus a breakpoint on a known
/// line inside the SECOND task body. The cooperative debugger must pause exactly
/// at that line, then Continue must run both tasks + the `async/all` to
/// completion.
#[test]
fn coop_async_two_tasks_breakpoint_stops_at_known_line() {
    // 1  (define a (async/spawn (fn ()
    // 2    (* 2 3))))
    // 3  (define b (async/spawn (fn ()
    // 4    (+ 10 20))))           <- breakpoint here, inside task b
    // 5  (async/all (list a b))
    let source = "(define a (async/spawn (fn ()\n  (* 2 3))))\n\
                  (define b (async/spawn (fn ()\n  (+ 10 20))))\n\
                  (async/all (list a b))\n";
    let (mut coop, first) = Coop::start(source, 4);
    assert_stopped_at(&first, 4);
    coop.continue_to_finish();
}

/// A breakpoint inside the FIRST of two tasks — proves the pause location is the
/// task that actually hits the line, and Continue still finishes the whole
/// `async/all`.
#[test]
fn coop_async_breakpoint_in_first_task() {
    let source = "(define a (async/spawn (fn ()\n  (* 2 3))))\n\
                  (define b (async/spawn (fn ()\n  (+ 10 20))))\n\
                  (async/all (list a b))\n";
    let (mut coop, first) = Coop::start(source, 2);
    assert_stopped_at(&first, 2);
    coop.continue_to_finish();
}

/// Regression: a breakpoint inside a HOF callback (`map`) running in an async
/// task must complete cleanly. On the runtime path the callback may itself pause
/// at the breakpoint (an UPGRADE over the legacy auto-continue) or run through
/// without a separate stop; either way Continue must drive it to `(2 4 6)` and
/// never surface a "HOF callback did not complete" error.
#[test]
fn coop_breakpoint_in_hof_callback_in_async_task_completes() {
    // 1  (define p (async/spawn (fn ()
    // 2    (map (fn (x)
    // 3      (* x 2))          <- breakpoint inside the HOF callback
    // 4      (list 1 2 3)))))
    // 5  (await p)
    let source =
        "(define p (async/spawn (fn ()\n  (map (fn (x)\n    (* x 2))\n    (list 1 2 3)))))\n(await p)\n";
    let (mut coop, first) = Coop::start(source, 3);
    match first {
        VmExecResult::Finished(v) => {
            assert_eq!(format!("{v}"), "(2 4 6)", "map result should be (2 4 6)");
        }
        // Any intermediate stop must Continue cleanly to the final result.
        _ => coop.continue_to_finish(),
    }
}

/// Cross-scheduler stepping with task-correct depth: StepOver and StepOut at a
/// stop INSIDE an async task must use the PAUSED TASK's frame depth. The task
/// body is a single frame (depth 1); the main thread awaits from a deeper frame.
/// Measuring against the task's VM (1) keeps StepOver advancing line-by-line
/// within the task and StepOut leaving it.
#[test]
fn coop_async_step_over_and_out_use_task_depth() {
    // 1  (define p (async/spawn (fn ()
    // 2    (let ((a 1))            <- breakpoint; task frame depth 1
    // 3      (let ((b 2))
    // 4        (+ a b))))))
    // 5  (define (drive) (await p))   <- main awaits from depth 2
    // 6  (drive)
    let source = "(define p (async/spawn (fn ()\n  (let ((a 1))\n    (let ((b 2))\n      (+ a b))))))\n(define (drive) (await p))\n(drive)\n";

    // StepOver advances within the task: line 2 -> 3.
    {
        let (mut coop, first) = Coop::start(source, 2);
        assert_stopped_at(&first, 2);
        let next = coop.step(StepMode::StepOver);
        assert_stopped_at(&next, 3);
    }

    // StepOut leaves the task's only frame — with the depth taken from the task's
    // VM it must NOT stop again on line 3/4 inside the same task.
    {
        let (mut coop, first) = Coop::start(source, 2);
        assert_stopped_at(&first, 2);
        let after = coop.step(StepMode::StepOut);
        match after {
            VmExecResult::Stopped(info) => assert!(
                info.line < 2 || info.line > 4,
                "StepOut must not stop INSIDE the async task body (lines 2-4), got {info:?}"
            ),
            VmExecResult::Finished(_) | VmExecResult::Yielded | VmExecResult::AsyncYield(_) => {}
            VmExecResult::QuantumExpired { .. } | VmExecResult::Pending(_) => {
                unreachable!("cooperative debug stepping does not surface a runtime quantum")
            }
        }
    }
}

/// At an async stop, INSPECTION must read the PAUSED TASK's per-task VM — the task
/// whose frame is at the breakpoint, with its task-local in scope.
/// `Runtime::with_paused_task_vm` relocates that VM so
/// GetStackTrace/GetScopes/GetVariables target its frames.
#[test]
fn coop_async_stop_inspects_paused_task_locals() {
    // 1  (define p (async/spawn (fn ()
    // 2    (let ((n 42))
    // 3      (+ n 1)))))      <- breakpoint here, inside the task, n bound to 42
    // 4  (await p)
    let source = "(define p (async/spawn (fn ()\n  (let ((n 42))\n    (+ n 1)))))\n(await p)\n";
    let (mut coop, first) = Coop::start(source, 3);
    assert_stopped_at(&first, 3);

    // The paused task's VM sees its frame at the breakpoint line and the local n.
    let n_value = coop
        .interpreter
        .runtime()
        .with_paused_task_vm(|tvm| {
            let frames = tvm.debug_stack_trace();
            assert!(!frames.is_empty(), "paused task should have a frame");
            assert_eq!(
                frames[0].line, 3,
                "task top frame at breakpoint line: {frames:?}"
            );
            let fid = frames[0].id as usize;
            tvm.debug_variables(sema_vm::scope_locals_ref(fid))
                .into_iter()
                .find(|v| v.name == "n")
                .map(|v| v.value)
        })
        .expect("a task is paused at the cooperative stop")
        .expect("task-local `n` is in scope at line 3");
    assert_eq!(n_value, "42", "task-local n should be 42 at the async stop");

    coop.continue_to_finish();
}

/// Session-boundary hygiene: a cooperative session ABANDONED (Stop) while paused
/// at an async breakpoint must not poison the NEXT session on the same persistent
/// interpreter runtime. Without clearing the runtime-wide debug barrier and
/// cancelling the abandoned root, every future `drive` would freeze at the stale
/// `DebugStopped`. `Runtime::debug_cancel_paused` scrubs it.
#[test]
fn coop_abandoned_async_session_does_not_poison_next_session() {
    let interpreter = Interpreter::new();

    // SESSION A: pause inside an async task, then ABANDON (Stop while paused).
    {
        let (vm, mut debug) = build(
            &interpreter,
            "(define p (async/spawn (fn ()\n  (+ 1 2))))\n(await p)\n",
            &[2],
        );
        let handle = interpreter
            .runtime()
            .submit_root(vm)
            .expect("root A submits");
        let first = drive_debug(&interpreter, &mut debug, &handle);
        assert_stopped_at(&first, 2);
        assert!(
            interpreter.runtime().is_debug_paused(),
            "session A must be paused at the async breakpoint"
        );
        // Simulate the Stop button: clear the barrier + cancel the abandoned root.
        assert!(
            interpreter.runtime().debug_cancel_paused(),
            "abandon must clear a live debug barrier"
        );
        // Drive the cancellation to settlement so no task lingers.
        let budget = DriveBudget::host_default();
        for _ in 0..10_000 {
            if interpreter.runtime_live_task_count() == 0 {
                break;
            }
            if !matches!(
                interpreter.runtime().drive(&budget),
                Ok(DriveState::Progress { .. })
            ) {
                break;
            }
        }
        // Drop vm/debug/handle here.
    }
    assert!(
        !interpreter.runtime().is_debug_paused(),
        "the abandoned barrier must be cleared"
    );
    assert_eq!(
        interpreter.runtime_live_task_count(),
        0,
        "the abandoned session must leave no lingering task in the reused runtime"
    );

    // SESSION B: a fresh program on the SAME interpreter must run cleanly to its
    // OWN result (42), not corruption.
    {
        let (vm, mut debug) = build(&interpreter, "(define r (* 6 7))\nr\n", &[1]);
        let handle = interpreter
            .runtime()
            .submit_root(vm)
            .expect("root B submits");
        let first = drive_debug(&interpreter, &mut debug, &handle);
        assert_stopped_at(&first, 1);

        for _ in 0..1_000 {
            if interpreter.runtime().is_debug_paused() {
                debug.step_mode = StepMode::Continue;
                interpreter.runtime().debug_resume();
            }
            if let VmExecResult::Finished(v) = drive_debug(&interpreter, &mut debug, &handle) {
                assert_eq!(format!("{v}"), "42", "session B must yield its own result");
                return;
            }
        }
        panic!("session B did not finish cleanly");
    }
}
