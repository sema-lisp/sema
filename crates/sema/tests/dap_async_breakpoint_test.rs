//! Gate: breakpoints fire INSIDE async tasks under the native DAP debugger.
//!
//! Root cause (pre-fix): breakpoint/step checking only runs in
//! `VM::run_inner(ctx, Some(debug))`, but the cooperative scheduler steps every
//! async task via the NON-debug `execute_async`/`run_async` path, so a breakpoint
//! on a line that executes only inside `(async/spawn (fn () …))` is silently
//! skipped. Fixed; see `docs/plans/archive/2026-06-23-async-debugger.md` (the
//! residual cross-task stepping gap is tracked as ASYNC-2 in `docs/deferred.md`).
//!
//! This is a fast MECHANISM-level test (it drives the unified runtime debug path
//! directly, NOT the slow binary-protocol DAP harness). It mirrors the native DAP
//! run setup in `crates/sema-dap/src/server.rs`: read_many_with_spans →
//! compile_program_with_spans → DebugState::new + set_valid_breakpoint_lines + set
//! breakpoint → `ActiveDebugGuard::enter(&mut ds)` → `drive_vm_on_runtime` on a
//! spawned thread (the debug quantum blocks on command_rx at a stop).

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use sema_eval::Interpreter;
use sema_vm::{DebugCommand, DebugEvent, DebugState, StopReason, VM};

/// Compile `source` (attributing spans to `path`), build a VM + DebugState with a
/// breakpoint on `bp_line`, and run it on a spawned thread. Returns the receiver
/// half of the event channel and the command sender so the test thread can drive
/// the stop/resume handshake, plus the join handle for the run.
struct DebugRun {
    event_rx: mpsc::Receiver<DebugEvent>,
    cmd_tx: mpsc::Sender<DebugCommand>,
    handle: std::thread::JoinHandle<Result<(), String>>,
}

fn start_debug_run(source: &str, path: &Path, bp_line: u32) -> DebugRun {
    // VM → frontend events, frontend → VM commands.
    let (event_tx, event_rx) = mpsc::channel::<DebugEvent>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<DebugCommand>();

    // Everything that touches the `Rc`-laden VM/Interpreter must be built and run
    // on the SAME thread (those types are `!Send`). So we move only the `Send`
    // inputs (source, path, channel halves) into the spawned thread and do the
    // full DAP-style setup there. The debug quantum blocks on command_rx at each
    // stop, so the drive must run off the test thread regardless.
    let source = source.to_string();
    let path = path.to_path_buf();
    let handle = std::thread::spawn(move || -> Result<(), String> {
        let (vals, span_map) =
            sema_reader::read_many_with_spans(&source).map_err(|e| e.to_string())?;
        let prog = sema_vm::compile_program_with_spans(&vals, &span_map, Some(path.clone()))
            .map_err(|e| e.to_string())?;

        let interpreter = Interpreter::new();

        let mut ds = DebugState::new(event_tx, cmd_rx);
        ds.set_valid_breakpoint_lines(sema_vm::valid_breakpoint_lines_by_file(
            &prog.closure,
            &prog.functions,
        ));
        let resolved = ds.set_breakpoints(&path, &[bp_line]);
        if !resolved.iter().any(|bp| bp.verified) {
            return Err(format!(
                "breakpoint on line {bp_line} did not resolve: {resolved:?}"
            ));
        }

        let mut vm = VM::new(
            interpreter.global_env.clone(),
            prog.functions,
            &[],
            prog.main_cache_slots,
        )
        .map_err(|e| e.to_string())?;
        vm.seed_main_frame(prog.closure.clone());

        // Register the DebugState as the active session for the drive: the
        // runtime's `run_parked_quantum` runs the debug-aware quantum, so a
        // breakpoint inside an async task stops and serves inspection against the
        // stopped task's own VM. Async/spawn + await are ordinary runtime
        // suspensions now, so they work under the debugger without a scheduler.
        let _active = sema_vm::ActiveDebugGuard::enter(&mut ds);
        interpreter
            .drive_vm_on_runtime(vm)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });

    DebugRun {
        event_rx,
        cmd_tx,
        handle,
    }
}

/// Control case: a breakpoint on a SYNCHRONOUS top-level line stops. Proves the
/// harness is valid (so a failure of the async case is a real gap, not a broken
/// test).
#[test]
fn sync_breakpoint_stops() {
    let dir = std::env::temp_dir().join(format!("sema-dap-sync-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sync.sema");
    let source = "(define x 1)\n(define y (+ x 2))\n(+ x y)\n";
    std::fs::write(&path, source).unwrap();
    let path = std::fs::canonicalize(&path).unwrap();

    // Breakpoint on line 2: `(define y (+ x 2))`.
    let run = start_debug_run(source, &path, 2);

    let evt = run
        .event_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a Stopped event should arrive for the sync breakpoint");
    assert!(
        matches!(
            evt,
            DebugEvent::Stopped {
                reason: StopReason::Breakpoint,
                ..
            }
        ),
        "expected Stopped(Breakpoint), got {evt:?}"
    );

    run.cmd_tx.send(DebugCommand::Continue).unwrap();
    run.handle
        .join()
        .expect("run thread joins")
        .expect("program runs to completion");
    let _ = std::fs::remove_dir_all(&dir);
}

/// THE GATE: a breakpoint on a line that runs only INSIDE an async task stops, and
/// Continue resumes to completion.
#[test]
fn async_task_breakpoint_stops_and_continues() {
    let dir = std::env::temp_dir().join(format!("sema-dap-async-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("async.sema");
    // Breakpoint on line 2: `(+ 1 2)` — runs only inside the spawned task body.
    let source = "(define p (async/spawn (fn ()\n  (+ 1 2))))\n(await p)\n";
    std::fs::write(&path, source).unwrap();
    let path = std::fs::canonicalize(&path).unwrap();

    let run = start_debug_run(source, &path, 2);

    let evt = run
        .event_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a Stopped event should arrive for the async-task breakpoint");
    assert!(
        matches!(
            evt,
            DebugEvent::Stopped {
                reason: StopReason::Breakpoint,
                ..
            }
        ),
        "expected Stopped(Breakpoint) inside the async task, got {evt:?}"
    );

    run.cmd_tx.send(DebugCommand::Continue).unwrap();
    run.handle
        .join()
        .expect("run thread joins")
        .expect("program runs to completion after Continue");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Slice-1 follow-up: at a breakpoint INSIDE an async task, the inspection
/// commands (GetStackTrace / GetScopes / GetVariables) must target the STOPPED
/// TASK's VM frames — so the task-local `n` is visible with its value — not the
/// main VM's frames (which are parked at `(await p)` with no `n` in scope).
///
/// This pins the wiring that `handle_debug_stop` runs on `task.vm`: had it served
/// inspection against the main VM, `n` would be absent.
#[test]
fn async_task_breakpoint_inspects_task_frame_locals() {
    let dir = std::env::temp_dir().join(format!("sema-dap-async-insp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("async_insp.sema");
    // Line 3: `(+ n 1)` runs only inside the task, where `n` is bound to 42.
    let source = "(define p (async/spawn (fn ()\n  (let ((n 42))\n    (+ n 1)))))\n(await p)\n";
    std::fs::write(&path, source).unwrap();
    let path = std::fs::canonicalize(&path).unwrap();

    let run = start_debug_run(source, &path, 3);

    let evt = run
        .event_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a Stopped event should arrive for the async-task breakpoint");
    assert!(
        matches!(evt, DebugEvent::Stopped { .. }),
        "expected Stopped inside the async task, got {evt:?}"
    );

    // GetStackTrace must report the task's frame (the inner `fn`), not the main
    // top-level frame — the top frame's line is the breakpoint line (3).
    let (tx, rx) = mpsc::sync_channel(1);
    run.cmd_tx
        .send(DebugCommand::GetStackTrace { reply: tx })
        .unwrap();
    let frames = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("stack trace reply");
    assert!(!frames.is_empty(), "expected at least one task frame");
    let top = &frames[0];
    assert_eq!(
        top.line, 3,
        "top frame should be at the breakpoint line: {frames:?}"
    );
    let frame_id = top.id as usize;

    // GetScopes → the Locals scope reference for that frame.
    let (tx, rx) = mpsc::sync_channel(1);
    run.cmd_tx
        .send(DebugCommand::GetScopes {
            frame_id,
            reply: tx,
        })
        .unwrap();
    let scopes = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("scopes reply");
    let locals_ref = scopes
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case("locals"))
        .map(|s| s.variables_reference)
        .expect("a Locals scope should exist");

    // GetVariables on the Locals scope must surface the task-local `n = 42`.
    let (tx, rx) = mpsc::sync_channel(1);
    run.cmd_tx
        .send(DebugCommand::GetVariables {
            reference: locals_ref,
            reply: tx,
        })
        .unwrap();
    let vars = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("variables reply");
    assert!(
        vars.iter().any(|v| v.name == "n" && v.value == "42"),
        "task-local `n = 42` should be visible at the async stop, got {vars:?}"
    );

    run.cmd_tx.send(DebugCommand::Continue).unwrap();
    run.handle
        .join()
        .expect("run thread joins")
        .expect("program runs to completion after Continue");
    let _ = std::fs::remove_dir_all(&dir);
}
