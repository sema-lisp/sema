//! Gate (Slice 2): breakpoints INSIDE async tasks STOP + CONTINUE in the
//! COOPERATIVE (WASM playground) debugger — `VM::start_cooperative` /
//! `run_cooperative`, NOT the blocking native `execute_debug`.
//!
//! This replicates the cooperative WASM flow WITHOUT a browser, mirroring
//! `SemaPlayground::debug_start` in `crates/sema-wasm/src/lib.rs`:
//! read_many_with_spans → compile_program_with_spans → DebugState::new_headless
//! set_valid_breakpoint_lines + set_breakpoints → init_scheduler →
//! start_cooperative, then run_cooperative to simulate Continue.
//!
//! Why the native gate (`dap_async_breakpoint_test.rs`) is not enough: the
//! cooperative path is step-driven and must NOT block on a command channel — it
//! RETURNS `VmExecResult::Stopped(info)` to JS and resumes via a later
//! `run_cooperative`. Slice 1 fixed only the blocking native path.
//!
//! Contract under test: `start_cooperative` returns `Stopped` whose info line ==
//! the async breakpoint line (control: a SYNC-line breakpoint already does this —
//! it proves the harness). A follow-up `run_cooperative` (simulating Continue)
//! eventually returns `Finished`.

#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;

use sema_eval::Interpreter;
use sema_vm::{DebugState, StepMode, VmExecResult, VM};

/// One cooperative debug session: the VM, its (headless) DebugState, and the
/// interpreter whose `ctx`/`global_env` it runs against. Built by [`start`].
struct Coop {
    vm: VM,
    debug: DebugState,
    interpreter: Interpreter,
}

impl Coop {
    /// Compile `source` cooperatively (spans → `<playground>`, matching the WASM
    /// path), install the scheduler, set a single breakpoint on `bp_line`, and
    /// start cooperative execution. Returns the session plus the first result.
    fn start(source: &str, bp_line: u32) -> (Self, VmExecResult) {
        let interpreter = Interpreter::new();
        let (vals, span_map) = sema_reader::read_many_with_spans(source).expect("parses");
        // The WASM debugger attributes spans to this synthetic path; canonicalize
        // fails for it everywhere, so both the compiler and set_breakpoints keep
        // it verbatim and agree on the breakpoint key.
        let source_file = PathBuf::from("<playground>");
        let prog = sema_vm::compile_program_with_spans(&vals, &span_map, Some(source_file.clone()))
            .expect("compiles");
        sema_vm::init_scheduler(interpreter.global_env.clone(), Vec::new());

        let valid = sema_vm::valid_breakpoint_lines(&prog.closure, &prog.functions);
        let snapped =
            sema_vm::snap_breakpoint_line(bp_line, &valid).expect("bp line snaps to executable");
        assert_eq!(
            snapped, bp_line,
            "test programs use directly-executable breakpoint lines"
        );

        let mut debug = DebugState::new_headless();
        debug.set_valid_breakpoint_lines(sema_vm::valid_breakpoint_lines_by_file(
            &prog.closure,
            &prog.functions,
        ));
        let resolved = debug.set_breakpoints(&source_file, &[snapped]);
        assert!(
            resolved.iter().any(|bp| bp.verified),
            "breakpoint on line {bp_line} did not resolve: {resolved:?}"
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

        let first = vm
            .start_cooperative(prog.closure.clone(), &interpreter.ctx, &mut debug)
            .expect("cooperative start does not error");

        (
            Coop {
                vm,
                debug,
                interpreter,
            },
            first,
        )
    }

    /// Apply a step command at the current stop and run one cooperative slice,
    /// mirroring `WasmInterpreter::debug_resume`: at an async stop the step depth
    /// is measured against the PAUSED TASK's VM (the one that resumes), not the
    /// main VM parked at the await.
    fn step(&mut self, mode: StepMode) -> VmExecResult {
        self.debug.step_mode = mode;
        if mode != StepMode::Continue {
            self.debug.step_frame_depth =
                sema_vm::with_coop_paused_task_vm(|tvm| tvm.frame_count())
                    .unwrap_or_else(|| self.vm.frame_count());
        }
        self.debug.instructions_remaining = 5_000_000;
        self.vm
            .run_cooperative(&self.interpreter.ctx, &mut self.debug)
            .expect("run_cooperative does not error on step")
    }

    /// Simulate a Continue / poll loop: re-enter `run_cooperative` until the
    /// program finishes. Panics if it does not finish within the tick budget.
    fn continue_to_finish(&mut self) {
        for _ in 0..10_000 {
            self.debug.instructions_remaining = 5_000_000;
            if let VmExecResult::Finished(_) = self
                .vm
                .run_cooperative(&self.interpreter.ctx, &mut self.debug)
                .expect("run_cooperative does not error on resume")
            {
                return;
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
/// test). This already works today.
#[test]
fn coop_sync_breakpoint_full_cycle() {
    let (mut coop, first) = Coop::start("(define x 1)\n(define y (+ x 2))\n(+ x y)\n", 2);
    assert_stopped_at(&first, 2);
    coop.continue_to_finish();
}

/// THE GATE: a breakpoint on a line that runs only INSIDE an async task must
/// surface cooperatively as `Stopped` (line == the async breakpoint line), and a
/// follow-up `run_cooperative` (Continue) must drive it to `Finished`.
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md"]
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
/// at that line (not the first task's body, not the top level), then Continue
/// must run both tasks + the `async/all` to completion.
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md"]
fn coop_async_two_tasks_breakpoint_stops_at_known_line() {
    // Lines (1-indexed):
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

/// A breakpoint inside the FIRST of two tasks, with the breakpoint set on the
/// first task's body line — proves pause location is the task that actually hits
/// the line, and Continue still finishes the whole `async/all`.
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md"]
fn coop_async_breakpoint_in_first_task() {
    let source = "(define a (async/spawn (fn ()\n  (* 2 3))))\n\
                  (define b (async/spawn (fn ()\n  (+ 10 20))))\n\
                  (async/all (list a b))\n";
    let (mut coop, first) = Coop::start(source, 2);
    assert_stopped_at(&first, 2);
    coop.continue_to_finish();
}

/// Regression: a breakpoint inside a HOF callback (`map`) running in
/// an async task must NOT crash the cooperative session with "HOF callback did
/// not complete". The callback runs via `run_closure_as_inline_task` (the
/// in-async-context fallback) which drives the scheduler synchronously inside the
/// owning task's native call; it cannot suspend back to JS, so the cooperative
/// debugger auto-continues through the stop and the program completes. (Native
/// DAP still pauses there via the blocking path.)
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md"]
fn coop_breakpoint_in_hof_callback_in_async_task_completes() {
    // 1  (define p (async/spawn (fn ()
    // 2    (map (fn (x)
    // 3      (* x 2))          <- breakpoint inside the HOF callback (inline task)
    // 4      (list 1 2 3)))))
    // 5  (await p)
    let source = "(define p (async/spawn (fn ()\n  (map (fn (x)\n    (* x 2))\n    (list 1 2 3)))))\n(await p)\n";
    let (mut coop, first) = Coop::start(source, 3);
    // It must auto-continue through the inline-task stop and complete — never
    // surface the "HOF callback did not complete" error.
    match first {
        VmExecResult::Finished(v) => {
            assert_eq!(format!("{v}"), "(2 4 6)", "map result should be (2 4 6)");
        }
        // If it surfaces an intermediate stop/yield, Continue must still finish it
        // cleanly (and never with the "did not complete" error).
        _ => coop.continue_to_finish(),
    }
}

/// Slice-1 follow-up #1 (cross-scheduler stepping, task-correct depth): StepOver
/// and StepOut at a stop INSIDE an async task must use the TASK's frame depth, so
/// that — even when the main thread awaits from a deeper frame than the task —
/// StepOver advances line-by-line within the task and StepOut leaves the task
/// instead of erroneously stopping on the next line within it.
///
/// The main thread awaits from inside `(drive)` (main depth 2) while the task
/// body is a single frame (depth 1). Measuring the step depth against the main VM
/// (2) makes StepOut's `depth < step_frame_depth` (1 < 2) wrongly true and stops
/// inside the task; the depth must come from the paused task's VM (1).
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md"]
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
    // VM it must NOT stop again on line 3/4 inside the same task; the task runs out.
    {
        let (mut coop, first) = Coop::start(source, 2);
        assert_stopped_at(&first, 2);
        let after = coop.step(StepMode::StepOut);
        match after {
            // Correct: control left the task; nothing in the task stops again.
            // (It finishes, or yields back to the main poll loop — either way it
            // must not be a Step stop on a line still inside the task body.)
            VmExecResult::Stopped(info) => assert!(
                info.line < 2 || info.line > 4,
                "StepOut must not stop INSIDE the async task body (lines 2-4), got {info:?}"
            ),
            VmExecResult::Finished(_) | VmExecResult::Yielded | VmExecResult::AsyncYield(_) => {}
            VmExecResult::QuantumExpired { .. } | VmExecResult::Pending(_) => {
                unreachable!("debug stepping does not install a runtime quantum")
            }
        }
    }
}

/// Slice-1 follow-up #2 (cooperative half): at an async stop, INSPECTION must
/// read the PAUSED TASK's per-task VM — not the main VM, which is parked at
/// `(await p)` and has no task-local in scope. `with_coop_paused_task_vm`
/// relocates the paused task by id so GetStackTrace/GetScopes/GetVariables target
/// its frames.
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md"]
fn coop_async_stop_inspects_paused_task_locals() {
    // 1  (define p (async/spawn (fn ()
    // 2    (let ((n 42))
    // 3      (+ n 1)))))      <- breakpoint here, inside the task, n bound to 42
    // 4  (await p)
    let source = "(define p (async/spawn (fn ()\n  (let ((n 42))\n    (+ n 1)))))\n(await p)\n";
    let (mut coop, first) = Coop::start(source, 3);
    assert_stopped_at(&first, 3);

    // The paused task's VM sees its frame at the breakpoint line and the local n.
    let n_value = sema_vm::with_coop_paused_task_vm(|tvm| {
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

    // CONTRAST: the MAIN VM (what naive inspection would read) is parked at the
    // await and has no `n` — proving the routing to the task VM was necessary.
    let main_has_n = coop
        .vm
        .debug_stack_trace()
        .first()
        .map(|f| f.id as usize)
        .map(|fid| {
            coop.vm
                .debug_variables(sema_vm::scope_locals_ref(fid))
                .iter()
                .any(|v| v.name == "n")
        })
        .unwrap_or(false);
    assert!(
        !main_has_n,
        "main VM must NOT expose the task-local `n` (it would mean inspection hit the wrong VM)"
    );

    coop.continue_to_finish();
}

/// Regression (session-boundary hygiene): a cooperative session that
/// is ABANDONED (Stop) while paused at an async breakpoint must not poison the
/// NEXT session. The await native leaves a `DEBUG_COOP_RESUME` pending and the
/// paused task `Ready` in the (reused) scheduler; without cleanup the next
/// program's first Continue would consume the stale resume, re-drive the dead
/// target, and clobber the new VM's stack. `start_cooperative` clears that state,
/// so session B runs cleanly.
///
/// Both sessions share ONE interpreter (so the scheduler is reused and the
/// thread-locals persist), mirroring the persistent WASM playground interpreter.
#[test]
#[ignore = "async debugging pending runtime cooperative-debug mode — see docs/deferred.md"]
fn coop_abandoned_async_session_does_not_poison_next_session() {
    let interpreter = Interpreter::new();
    sema_vm::init_scheduler(interpreter.global_env.clone(), Vec::new());
    let source_file = PathBuf::from("<playground>");

    // Build a fresh VM + headless DebugState (single breakpoint) for `source`,
    // sharing `interpreter` — i.e. NOT re-initialising the scheduler.
    let build = |source: &str, bp_line: u32| {
        let (vals, span_map) = sema_reader::read_many_with_spans(source).expect("parses");
        let prog = sema_vm::compile_program_with_spans(&vals, &span_map, Some(source_file.clone()))
            .expect("compiles");
        let mut debug = DebugState::new_headless();
        debug.set_valid_breakpoint_lines(sema_vm::valid_breakpoint_lines_by_file(
            &prog.closure,
            &prog.functions,
        ));
        let resolved = debug.set_breakpoints(&source_file, &[bp_line]);
        assert!(
            resolved.iter().any(|bp| bp.verified),
            "bp {bp_line} resolves"
        );
        debug.step_mode = StepMode::Continue;
        debug.instructions_remaining = 5_000_000;
        let vm = VM::new(
            interpreter.global_env.clone(),
            prog.functions,
            &[],
            prog.main_cache_slots,
        )
        .expect("VM builds");
        (vm, debug, prog.closure)
    };

    // SESSION A: pause inside an async task, then ABANDON (drop without resuming).
    {
        let (mut vm, mut debug, closure) = build(
            "(define p (async/spawn (fn ()\n  (+ 1 2))))\n(await p)\n",
            2,
        );
        let first = vm
            .start_cooperative(closure, &interpreter.ctx, &mut debug)
            .expect("session A starts");
        assert_stopped_at(&first, 2);
        // Drop vm/debug here WITHOUT resuming — simulates the user clicking Stop
        // while paused. This leaves DEBUG_COOP_RESUME pending + the paused task
        // Ready in the reused scheduler.
    }

    // The leak source is real: the await native left a resume pending and the
    // paused task is still parked in the scheduler. (Sanity — if these were
    // already clean the regression would be vacuous.)
    assert!(
        sema_core::debug_coop_resume_pending(),
        "session A should have left a pending DEBUG_COOP_RESUME (the leak source)"
    );
    assert!(
        sema_vm::scheduler_task_count() >= 1,
        "session A's paused task should still be parked in the reused scheduler"
    );

    // SESSION B: starting a fresh program on the SAME interpreter must SCRUB that
    // leaked state, so the next Continue cannot consume a stale resume / re-drive
    // a dead target, and no abandoned task lingers.
    {
        let (mut vm, mut debug, closure) = build("(define r (* 6 7))\nr\n", 1);
        let first = vm
            .start_cooperative(closure, &interpreter.ctx, &mut debug)
            .expect("session B starts");
        assert_stopped_at(&first, 1);

        // Session B start scrubs the leaked state: no stale resume, no leftover
        // task carried over from the abandoned session A.
        assert!(
            !sema_core::debug_coop_resume_pending(),
            "session B start must clear the stale DEBUG_COOP_RESUME"
        );
        assert_eq!(
            sema_vm::scheduler_task_count(),
            0,
            "session B start must drop the abandoned task from the reused scheduler"
        );

        // And it runs to its OWN result (42), not corruption.
        let mut result = first;
        for _ in 0..1000 {
            debug.instructions_remaining = 5_000_000;
            result = vm
                .run_cooperative(&interpreter.ctx, &mut debug)
                .expect("session B Continue does not error on a stale resume");
            if let VmExecResult::Finished(v) = &result {
                assert_eq!(format!("{v}"), "42", "session B must yield its own result");
                return;
            }
        }
        panic!("session B did not finish cleanly: {result:?}");
    }
}
