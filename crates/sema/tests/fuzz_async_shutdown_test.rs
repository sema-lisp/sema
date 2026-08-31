//! Shutdown-leak harness over the async grammar fuzzer — phase 3 of the plan
//! `docs/plans/archive/2026-07-29-async-grammar-fuzzer.md`.
//!
//! Each test generates async programs with the fuzzer's emit mode
//! (`SEMA_FUZZ_MODE=emit SEMA_FUZZ_ASYNC=1`; emit runs no oracle and consumes
//! the same RNG stream as check mode, so the seed-to-program mapping is
//! identical), evals them in-process, and then asserts the runtime tears down
//! clean: zero live tasks, resource gates back to their pre-eval baseline, and
//! `Interpreter::shutdown` reporting `clean` with no invariant failures. The
//! value/twin oracles are check mode's job (`scripts/grammar-fuzz.sh`); this
//! harness owns the Rust-side introspection the `.sema` fuzzer cannot reach.
//!
//! Two drive modes:
//!
//! - `fuzz_async_shutdown_each_seed_clean`: one fresh interpreter per seed,
//!   default `drive()` path (`eval_str_compiled`) — the native CLI shape.
//! - `fuzz_async_shutdown_paired_roots_drive_roots_clean`: seed pairs (A, B)
//!   on one shared interpreter, each root driven with the selection-scoped
//!   `drive_roots` — the wasm production shape, and the reproduction recipe
//!   for the v1.31.1 orphaned-pending-stage bug: leftover state from A, an
//!   undriven wall-clock gap, then B whose final form is an `async/sleep`.
//!   A timer wedged by A's leftovers hangs B; the per-root deadline turns
//!   that hang into a failure that names both seeds.
//!
//! Nightly-scoped: `#[ignore]` by default. Run explicitly with
//! `jake fuzz.async-shutdown` or
//! `cargo nextest run -p sema-lang --run-ignored ignored-only fuzz_async_shutdown`.
//! `SEMA_FUZZ_SEED` (base), `SEMA_FUZZ_COUNT`, and `SEMA_FUZZ_DEPTH` come from
//! the environment so the nightly job can rotate the base while every failure
//! stays reproducible from the logged values.

use std::rc::Rc;
use std::time::{Duration, Instant};

use sema_core::runtime::{RootId, TaskOutcome, TaskSettlement};
use sema_eval::Interpreter;
use sema_vm::runtime::{
    DriveBudget, DriveState, RootHandle, RootOptions, RootPoll, ShutdownOptions,
};

/// Wall-clock deadline per driven root. Purely a hang net (the plan's rule:
/// only a lower bound on progress, never a performance bound) — generated
/// programs settle in milliseconds.
const PER_ROOT_BUDGET: Duration = Duration::from_secs(30);

/// Wall-clock bound on draining leftover detached tasks after the root
/// settles. Generated detached children are value-neutral sleeps of at most a
/// few ms; anything still live after this grace is a leak.
const DRAIN_BUDGET: Duration = Duration::from_secs(10);

/// The undriven gap between A settling and B being submitted in paired mode —
/// long enough for A's leftover timer deadlines (sleeps are <= 5 ms) to fall
/// due while nothing drives, which is the state the v1.31.1 bug needed.
const UNDRIVEN_GAP: Duration = Duration::from_millis(20);

struct Config {
    base: u64,
    count: u64,
    depth: u64,
}

impl Config {
    fn from_env() -> Self {
        let get = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(default)
        };
        let cfg = Self {
            base: get("SEMA_FUZZ_SEED", 0),
            count: get("SEMA_FUZZ_COUNT", 100),
            depth: get("SEMA_FUZZ_DEPTH", 4),
        };
        // Every failure must reproduce from these three numbers, so print them
        // unconditionally (nextest keeps output for failed tests).
        eprintln!(
            "fuzz-async-shutdown: seed={} count={} depth={}",
            cfg.base, cfg.count, cfg.depth
        );
        cfg
    }
}

/// The repro line for one seed, in the same shape `scripts/grammar-fuzz.sh`
/// prints for its findings.
fn repro(seed: u64, depth: u64) -> String {
    format!(
        "SEMA_FUZZ_ASYNC=1 SEMA_FUZZ_MODE=emit SEMA_FUZZ_SEED={seed} SEMA_FUZZ_COUNT=1 \
         SEMA_FUZZ_DEPTH={depth} <sema> fuzz/grammar-fuzz.sema"
    )
}

/// Generate `count` programs (seeds `base..base+count`) in one emit-mode
/// subprocess of the real `sema` binary. Emit resets the per-iteration state
/// (`*cur-seed*`, `*file-ctr*`, RNG) exactly like check mode, so each entry is
/// the program check mode would run for that seed.
fn generate_programs(base: u64, count: u64, depth: u64) -> Vec<(u64, String)> {
    let fuzzer = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/grammar-fuzz.sema");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg(fuzzer)
        .env("SEMA_FUZZ_MODE", "emit")
        .env("SEMA_FUZZ_ASYNC", "1")
        .env("SEMA_FUZZ_SEED", base.to_string())
        .env("SEMA_FUZZ_COUNT", count.to_string())
        .env("SEMA_FUZZ_DEPTH", depth.to_string())
        .env_remove("SEMA_FUZZ_OUT")
        .env_remove("SEMA_FUZZ_CRASH_FILE")
        .env_remove("SEMA_FUZZ_VERBOSE")
        .output()
        .expect("spawn sema in emit mode");
    assert!(
        output.status.success(),
        "emit mode failed (status {:?}):\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("emit output is UTF-8");
    let mut programs: Vec<(u64, String)> = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("; seed=") {
            let seed: u64 = rest
                .split(' ')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| panic!("unparseable emit header: {line}"));
            programs.push((seed, String::new()));
        } else if !line.trim().is_empty() {
            let (_, prog) = programs
                .last_mut()
                .expect("program line before any seed header");
            if !prog.is_empty() {
                prog.push('\n');
            }
            prog.push_str(line);
        }
    }
    assert_eq!(
        programs.len(),
        count as usize,
        "emit produced a different program count than requested"
    );
    programs
}

/// Drive one submitted root to settlement using ONLY the selection-scoped
/// `drive_roots` path (never `drive()`), mirroring the Idle handling of the
/// interpreter's own settlement loop: block on the inbox for external
/// completions and timers, settle a genuine deadlock. `what` names the seed(s)
/// in every failure message.
fn drive_root_scoped_to_settlement(
    interp: &Interpreter,
    handle: &RootHandle,
    what: &str,
) -> Rc<TaskSettlement> {
    let runtime = interp.runtime();
    let budget = DriveBudget::host_default();
    let deadline = Instant::now() + PER_ROOT_BUDGET;
    loop {
        match handle.poll_result() {
            RootPoll::Ready(settlement) => return settlement,
            RootPoll::Pending => {}
            RootPoll::Aborted(fault) => {
                panic!("{what}: root aborted with runtime fault: {fault:?}")
            }
            RootPoll::RuntimeDropped => panic!("{what}: runtime dropped while driving"),
            RootPoll::InvariantViolation => panic!("{what}: runtime invariant violation"),
        }
        assert!(
            Instant::now() < deadline,
            "{what}: HANG — root did not settle within {PER_ROOT_BUDGET:?} under drive_roots"
        );
        let state = runtime
            .drive_roots(&budget, &[handle.id()])
            .unwrap_or_else(|fault| panic!("{what}: drive_roots fault: {fault:?}"));
        match state {
            DriveState::Progress { .. } => {}
            DriveState::Idle {
                next_deadline,
                inbox_wakeup_required: true,
            } => {
                let bound = next_deadline.map_or(deadline, |d| d.min(deadline));
                runtime.block_on_inbox(Some(bound));
            }
            DriveState::Idle {
                next_deadline: Some(timer),
                inbox_wakeup_required: false,
            } => {
                runtime.block_on_inbox(Some(timer.min(deadline)));
            }
            DriveState::Idle {
                next_deadline: None,
                inbox_wakeup_required: false,
            } => {
                // Nothing can ever wake the selected root: a genuine deadlock.
                // Generated programs terminate by construction, so reaching
                // this is itself a finding; settle it so the failure carries
                // the deadlock error rather than a bare hang.
                let settled = runtime
                    .settle_deadlocked_root(handle.id())
                    .unwrap_or_else(|fault| {
                        panic!("{what}: settle_deadlocked_root fault: {fault:?}")
                    });
                assert!(settled, "{what}: fully idle but root would not settle");
            }
            other => panic!("{what}: unexpected drive state while pending: {other:?}"),
        }
    }
}

/// After the root(s) settled, give leftover detached tasks (value-neutral
/// sleeps by construction) a bounded window to finish: drive until the
/// runtime stops making progress and no timer deadline remains. `roots`
/// selects the scoped path; `None` drives unscoped.
fn drain_leftovers(interp: &Interpreter, roots: Option<&[RootId]>, what: &str) {
    let runtime = interp.runtime();
    let budget = DriveBudget::host_default();
    let deadline = Instant::now() + DRAIN_BUDGET;
    while Instant::now() < deadline {
        let state = match roots {
            Some(ids) => runtime.drive_roots(&budget, ids),
            None => runtime.drive(&budget),
        }
        .unwrap_or_else(|fault| panic!("{what}: drive fault while draining leftovers: {fault:?}"));
        match state {
            DriveState::Progress { .. } => {}
            DriveState::Idle {
                next_deadline: Some(timer),
                ..
            } => {
                runtime.block_on_inbox(Some(timer.min(deadline)));
            }
            DriveState::Idle {
                next_deadline: None,
                inbox_wakeup_required: true,
            } => {
                runtime.block_on_inbox(Some(deadline));
            }
            // No ready work and no reported deadline. `Quiescent` (all root
            // records released) does not surface a leftover detached task's
            // pending timer, so while tasks are still live keep polling on a
            // short sleep — a due timer then fires on a later turn. If the
            // live count never reaches zero the caller's assert reports the
            // leak.
            DriveState::Idle {
                next_deadline: None,
                inbox_wakeup_required: false,
            }
            | DriveState::Quiescent => {
                if interp.runtime_live_task_count() == 0 {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            other => panic!("{what}: unexpected drive state while draining: {other:?}"),
        }
    }
}

/// Shut the interpreter down and assert the report is clean. Also re-checks
/// the two live counters afterwards so a report/counter disagreement is a
/// failure in its own right.
fn assert_clean_shutdown(interp: &Interpreter, gate_baseline: usize, what: &str) {
    let report = interp
        .shutdown(ShutdownOptions {
            deadline: Instant::now() + Duration::from_secs(5),
            drive_budget: DriveBudget::host_default(),
        })
        .unwrap_or_else(|e| panic!("{what}: shutdown errored: {e}"));
    assert!(
        report.clean && report.invariant_failures.is_empty(),
        "{what}: shutdown not clean: {report:?}"
    );
    assert_eq!(
        interp.runtime_live_task_count(),
        0,
        "{what}: live tasks after clean shutdown"
    );
    assert_eq!(
        interp.runtime_resource_gate_count(),
        gate_baseline,
        "{what}: resource gates not back to baseline after shutdown"
    );
}

/// One fresh interpreter per seed, default `drive()` path. Leak oracle: after
/// the program and a bounded leftover drain, zero live tasks and the resource
/// gates back to the pre-eval baseline BEFORE shutdown — then a clean
/// shutdown report on top.
#[test]
#[ignore = "nightly fuzz harness — run via jake fuzz.async-shutdown"]
fn fuzz_async_shutdown_each_seed_clean() {
    let cfg = Config::from_env();
    for (seed, program) in generate_programs(cfg.base, cfg.count, cfg.depth) {
        let what = format!("seed {seed} [{}]", repro(seed, cfg.depth));
        let interp = Interpreter::new();
        let gate_baseline = interp.runtime_resource_gate_count();
        interp
            .eval_str_compiled(&program)
            .unwrap_or_else(|e| panic!("{what}: eval failed: {e}\nprogram: {program}"));
        drain_leftovers(&interp, None, &what);
        assert_eq!(
            interp.runtime_live_task_count(),
            0,
            "{what}: tasks still live after settlement + drain\nprogram: {program}"
        );
        assert_eq!(
            interp.runtime_resource_gate_count(),
            gate_baseline,
            "{what}: resource gates not back to baseline\nprogram: {program}"
        );
        assert_clean_shutdown(&interp, gate_baseline, &what);
    }
}

/// Seed pairs (A, B) on one shared interpreter, every root driven with the
/// selection-scoped `drive_roots` — natively reproducing the wasm driving
/// shape. A's leftover detached state stays parked across an undriven gap;
/// B ends in an `async/sleep`, so a timer wedged by A's leftovers hangs B
/// and the per-root deadline reports both seeds.
#[test]
#[ignore = "nightly fuzz harness — run via jake fuzz.async-shutdown"]
fn fuzz_async_shutdown_paired_roots_drive_roots_clean() {
    let cfg = Config::from_env();
    let pair_count = (cfg.count / 2).max(1);
    let programs = generate_programs(cfg.base, pair_count * 2, cfg.depth);
    for pair in programs.as_chunks::<2>().0 {
        let (seed_a, prog_a) = &pair[0];
        let (seed_b, prog_b) = &pair[1];
        let what = format!(
            "pair ({seed_a}, {seed_b}) depth {} [replay both on one interpreter via drive_roots; \
             emit each with: {}]",
            cfg.depth,
            repro(*seed_a, cfg.depth)
        );
        let interp = Interpreter::new();
        let gate_baseline = interp.runtime_resource_gate_count();

        let root_a = interp
            .submit_str(prog_a, RootOptions::default())
            .unwrap_or_else(|e| panic!("{what}: submit A failed: {e}\nprogram A: {prog_a}"));
        let settled_a = drive_root_scoped_to_settlement(&interp, &root_a, &what);
        if let TaskOutcome::Failed(e) = &settled_a.outcome {
            panic!("{what}: root A failed: {e}\nprogram A: {prog_a}");
        }

        // The undriven gap: A's leftover timer deadlines fall due while
        // nothing drives. B is then submitted onto the same runtime.
        std::thread::sleep(UNDRIVEN_GAP);

        // The timer probe: B's root always ends parked on the timer wheel, so
        // a wheel wedged by A's leftover pending state hangs B here.
        let probed_b = format!("{prog_b}\n(async/sleep 2)");
        let root_b = interp
            .submit_str(&probed_b, RootOptions::default())
            .unwrap_or_else(|e| panic!("{what}: submit B failed: {e}\nprogram B: {prog_b}"));
        let settled_b = drive_root_scoped_to_settlement(&interp, &root_b, &what);
        if let TaskOutcome::Failed(e) = &settled_b.outcome {
            panic!("{what}: root B failed: {e}\nprogram B: {prog_b}");
        }

        // Leftover detached tasks from either root can only run under their
        // own root's selection — drain scoped to both.
        let ids = [root_a.id(), root_b.id()];
        drain_leftovers(&interp, Some(&ids), &what);
        assert_eq!(
            interp.runtime_live_task_count(),
            0,
            "{what}: tasks still live after both roots settled + drain\n\
             program A: {prog_a}\nprogram B: {prog_b}"
        );
        assert_eq!(
            interp.runtime_resource_gate_count(),
            gate_baseline,
            "{what}: resource gates not back to baseline\n\
             program A: {prog_a}\nprogram B: {prog_b}"
        );
        drop(root_a);
        drop(root_b);
        assert_clean_shutdown(&interp, gate_baseline, &what);
    }
}
