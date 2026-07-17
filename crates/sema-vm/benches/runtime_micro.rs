//! Divan micro-benchmarks for the unified cooperative runtime's hot scheduler
//! primitives (Task 0c-3, `docs/plans/2026-07-17-runtime-fast-path-0c.md`).
//!
//! Every Sema source form is read + compiled ONCE per benchmark, outside the
//! timed closure (`compile_once`/`spawn_and_park`), matching the "no source
//! parsing in the hot loop" requirement. Each timed iteration then builds a
//! fresh `VM` from the precompiled program (or, for the shutdown-sweep
//! benchmark, a fresh `Interpreter` built during divan's untimed
//! `with_inputs` setup phase) and drives it through a real `Runtime` exactly
//! the way the production interpreter does (`Interpreter::drive_vm_on_runtime`
//! / `Runtime::drive` / `Runtime::shutdown`).
//!
//! Every benchmark asserts on its result on every iteration (not just once at
//! setup): a benchmark that silently started measuring an error path, a
//! deadlocked root, or a task that never actually settled would be worse than
//! no benchmark at all, so each one panics loudly the moment that stops being
//! true. The assertions are cheap relative to the work being measured and are
//! wrapped in `divan::black_box` so the optimizer can't use them to prove the
//! rest of the computation is dead and elide it.
//!
//! This file only uses `sema_vm`'s public API (`pub mod runtime` re-exports)
//! plus `sema_eval::Interpreter` / `sema_reader` — nothing under
//! `crates/sema-vm/src/` was touched to build it.

use std::time::{Duration, Instant};

use sema_eval::Interpreter;
use sema_vm::runtime::{
    DriveBudget, DriveState, MonotonicClock, NullExecutor, Runtime, ShutdownOptions,
};
use sema_vm::{compile_program, CompiledProgram, VM};

fn main() {
    divan::main();
}

/// Read + compile a whole program (one or more top-level forms) exactly once.
/// Panics loudly at setup (never inside a timed closure) if the source
/// doesn't parse or compile — a benchmark must never silently measure a
/// program that failed to build.
fn compile_once(src: &str) -> CompiledProgram {
    let forms = sema_reader::read_many(src).expect("bench source parses");
    compile_program(&forms, None).expect("bench source compiles")
}

/// Build a fresh, idle `VM` seeded to run `prog`'s main chunk against
/// `interp`'s global env. Cheap (`Rc` clones only) — safe to call inside a
/// timed closure once per iteration, as the task spec requires ("fresh VM
/// instances per-iteration").
fn fresh_vm(interp: &Interpreter, prog: &CompiledProgram) -> VM {
    let mut vm = VM::new(
        interp.global_env.clone(),
        prog.functions.clone(),
        &prog.native_table,
        prog.main_cache_slots,
    )
    .expect("VM construction from a precompiled program cannot fail");
    vm.seed_main_frame(prog.closure.clone());
    vm
}

// ── (a) matched channel rendezvous ──────────────────────────────────────
//
// One root that creates a channel, spawns a child that sends on it, and
// receives. The spawned child does not run until the spawner (the root)
// suspends or returns, so the receive parks the root and the drive loop
// then runs the child to its send — a genuine send/receive rendezvous
// driven through the runtime, not an inline same-task fast path.

#[divan::bench]
fn channel_rendezvous(bencher: divan::Bencher) {
    let interp = Interpreter::new();
    let prog = compile_once(
        "(let ((ch (channel/new)))
           (async/spawn (fn () (channel/send ch 42)))
           (channel/recv ch))",
    );

    bencher.bench_local(|| {
        let vm = fresh_vm(&interp, &prog);
        let result = interp
            .drive_vm_on_runtime(vm)
            .expect("channel rendezvous settles Ok");
        assert_eq!(divan::black_box(&result), &sema_core::Value::int(42));
    });
}

// ── (b) spawn→settle lifecycle ──────────────────────────────────────────
//
// The bare cost of submitting a trivial root and driving it to `RootPoll::
// Ready` — no channels, timers, or callback dispatch, so this is the
// baseline task/root machinery cost the other scenarios build on top of.

#[divan::bench]
fn spawn_settle(bencher: divan::Bencher) {
    let interp = Interpreter::new();
    let prog = compile_once("(+ 1 1)");

    bencher.bench_local(|| {
        let vm = fresh_vm(&interp, &prog);
        let result = interp
            .drive_vm_on_runtime(vm)
            .expect("trivial root settles Ok");
        assert_eq!(divan::black_box(&result), &sema_core::Value::int(2));
    });
}

// ── (c) timer arm + fire ─────────────────────────────────────────────────
//
// `(async/sleep 0)`-shaped: arms a near-zero timer and drives until it fires
// and the root resumes and settles. Uses the interpreter's real
// `MonotonicClock`-backed runtime (a 0ms deadline is already-past by the time
// the drive loop next checks it, so this does not block on `thread::sleep`
// in practice, but the timer wheel arm/fire/wake path is fully exercised).

#[divan::bench]
fn timer_arm_and_fire(bencher: divan::Bencher) {
    let interp = Interpreter::new();
    let prog = compile_once("(async/sleep 0)");

    bencher.bench_local(|| {
        let vm = fresh_vm(&interp, &prog);
        let result = interp
            .drive_vm_on_runtime(vm)
            .expect("zero-duration sleep settles Ok");
        assert_eq!(divan::black_box(&result), &sema_core::Value::nil());
    });
}

// ── (d) in-place HOF element dispatch ────────────────────────────────────
//
// `map` over a 100-element list with a trivial closure. Every element call
// takes the cooperative `NativeOutcome::Call` continuation path (park parent
// VM, dispatch the callback VM, reinstall) — the path Slice 0b's "in-place
// HOF callback dispatch on a reused scratch VM" fix targeted, and the
// reference point for future regressions in per-element callback overhead.

const HOF_N: usize = 100;

fn hof_source() -> String {
    let items: Vec<String> = (1..=HOF_N as i64).map(|n| n.to_string()).collect();
    format!("(length (map (fn (x) (+ x 1)) (list {})))", items.join(" "))
}

#[divan::bench]
fn hof_map_100(bencher: divan::Bencher) {
    let interp = Interpreter::new();
    let prog = compile_once(&hof_source());

    bencher.bench_local(|| {
        let vm = fresh_vm(&interp, &prog);
        let result = interp
            .drive_vm_on_runtime(vm)
            .expect("map over 100 elements settles Ok");
        assert_eq!(
            divan::black_box(&result),
            &sema_core::Value::int(HOF_N as i64)
        );
    });
}

// ── (e) one idle drive turn ──────────────────────────────────────────────
//
// The cost of a `Runtime::drive` call that finds nothing to do: no roots, no
// tasks, no timers, no pending completions. A bare `Runtime` (not routed
// through `Interpreter`, which would carry stdlib-registration weight
// irrelevant to this measurement) built directly from the public
// `Runtime::new` / `NullExecutor` / `MonotonicClock` API.

#[divan::bench]
fn idle_drive_turn(bencher: divan::Bencher) {
    let ctx = std::rc::Rc::new(sema_core::EvalContext::new());
    let runtime = Runtime::new(
        ctx,
        std::rc::Rc::new(MonotonicClock),
        std::sync::Arc::new(NullExecutor),
    )
    .expect("fresh runtime construction cannot fail");
    let budget = DriveBudget::host_default();

    bencher.bench_local(|| {
        let state = runtime.drive(&budget).expect("idle drive turn succeeds");
        assert!(
            divan::black_box(matches!(
                state,
                DriveState::Idle { .. } | DriveState::Quiescent
            )),
            "expected an idle/quiescent turn on an empty runtime, got {state:?}"
        );
    });
}

// ── (f) cancel_waiting sweep over N parked tasks ─────────────────────────
//
// The only publicly reachable path to the private `cancel_waiting` scan is
// `Runtime::shutdown`, which cancels every live task and drains them via that
// same scan (see `Runtime::shutdown` in `runtime/state.rs`). Each iteration's
// setup (spawning and parking N children on an unbuffered channel that is
// never sent to, so every child blocks in `channel/recv`) runs in divan's
// untimed `with_inputs` phase; only `Runtime::shutdown` itself is timed.
// `shutdown` sets a permanent `shutting_down` flag, so a fresh `Interpreter`
// (and its fresh `Runtime`) is required per iteration — reusing one across
// iterations would time-degenerate after the first shutdown.

fn spawn_and_park(n: usize) -> Interpreter {
    let interp = Interpreter::new();
    let mut src = String::from("(define __bench_ch (channel/new))\n");
    for _ in 0..n {
        src.push_str("(async/spawn (fn () (channel/recv __bench_ch)))\n");
    }
    src.push('0');
    let result = interp
        .eval_str_via_runtime(&src)
        .expect("spawning N parked children settles the root Ok");
    assert_eq!(result, sema_core::Value::int(0));
    assert!(
        interp.runtime_live_task_count() >= n,
        "expected at least N={n} children parked on channel/recv after the root drains, got {}",
        interp.runtime_live_task_count()
    );
    interp
}

#[divan::bench(args = [0, 64])]
fn cancel_waiting_sweep(bencher: divan::Bencher, n: usize) {
    bencher
        .with_inputs(|| spawn_and_park(n))
        .bench_local_values(|interp| {
            let options = ShutdownOptions {
                deadline: Instant::now() + Duration::from_secs(5),
                drive_budget: DriveBudget::host_default(),
            };
            let report = interp
                .runtime()
                .shutdown(&options)
                .expect("shutdown drives to completion without a runtime fault");
            assert!(
                divan::black_box(report.clean),
                "expected a clean shutdown after cancelling {n} parked tasks, got {report:?}"
            );
        });
}
