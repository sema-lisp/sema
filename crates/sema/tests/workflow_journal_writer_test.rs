//! A3 — bounded workflow journal I/O off the VM thread.
//!
//! Every journal write happens on a per-run [`sema_workflow::writer::JournalWriter`]
//! thread; the VM thread only renders + `try_send`s. These tests pin the observable
//! contract:
//!
//! * a NORMAL `workflow/run` return means `events.jsonl` is complete on disk (the terminal
//!   flush-ack barrier ordering);
//! * cancelling a run parked on a STALLED writer settles promptly and leaves a runnable
//!   sibling (the flush-ack park is interruptible; a cancel never joins the writer);
//! * a full queue / an over-cap journal DROP + surface one `journal.overflow` marker,
//!   never blocking the VM thread;
//! * the four hard caps (`MEMO_MAX_COUNT`, `MEMO_FILE_MAX_BYTES`, `JOURNAL_TOTAL_MAX_BYTES`,
//!   rendered-value bytes) are enforced without materializing an over-cap value in full.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sema_core::Value;
use sema_eval::Interpreter;
use sema_workflow::context::{MEMO_FILE_MAX_BYTES, MEMO_MAX_COUNT};
use sema_workflow::event::WorkflowEvent;
use sema_workflow::{writer, Journal, WorkflowCtx};

// The `SEMA_WORKFLOW_*` env + the process-global writer stall gate are shared, so these
// tests serialize within this binary (separate binaries are separate processes).
static SERIAL: Mutex<()> = Mutex::new(());

/// Disarms the writer stall gate on drop so an early return / panic never leaves a later
/// test's writer stalled.
struct StallGuard;
impl Drop for StallGuard {
    fn drop(&mut self) {
        writer::__test_disarm_stall();
    }
}

fn read_events(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("valid event json"))
        .collect()
}

fn clear_env() {
    for v in [
        "SEMA_WORKFLOW_FIXED_TS",
        "SEMA_WORKFLOW_RUN_ID",
        "SEMA_WORKFLOW_RUN_DIR",
        "SEMA_WORKFLOW_JOURNAL_QUEUE",
        "SEMA_WORKFLOW_JOURNAL_MAX_BYTES",
    ] {
        std::env::remove_var(v);
    }
}

// ── (1) normal completion → complete journal at return (flush-ack ordering) ──

#[test]
fn normal_completion_flushes_complete_journal_at_return() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _stall = StallGuard;
    writer::__test_disarm_stall();
    clear_env();

    let dir = std::env::temp_dir().join(format!("sema-wfw-normal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_normal");
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &dir);

    let interp = Interpreter::new();
    let value = interp.eval_str_compiled(
        r#"
        (workflow/run "normal" "doc" {}
          (fn ()
            (workflow/checkpoint :a (fn () 1))
            {:status :success}))
        "#,
    );
    clear_env();
    assert!(value.is_ok(), "workflow ran: {value:?}");

    // The run returned; the terminal flush-ack barrier guarantees the writer flushed every
    // event to disk FIRST — so reading immediately sees a complete stream ending in
    // run.ended, with NO barrier wait of our own.
    let events = read_events(&dir.join("wf_normal").join("events.jsonl"));
    assert!(!events.is_empty(), "journal must not be empty at return");
    assert_eq!(events.first().unwrap()["event"], "run.started");
    assert_eq!(
        events.last().unwrap()["event"],
        "run.ended",
        "a normal return means run.ended is already on disk: {events:?}"
    );
    assert_eq!(events.last().unwrap()["status"], "success");
    // result.json is a sidecar that lands in the same barrier.
    assert!(dir.join("wf_normal").join("result.json").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

// ── (2) cancel on a stalled writer settles promptly + leaves a runnable sibling ──

#[test]
fn cancelled_run_on_stalled_writer_settles_and_leaves_runnable_sibling() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _stall = StallGuard;
    clear_env();

    let dir = std::env::temp_dir().join(format!("sema-wfw-stalled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_stalled");
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &dir);

    // Arm the stall BEFORE the run so its writer thread blocks before draining anything:
    // task A completes its body, enqueues run.ended + the Flush barrier, and PARKS on the
    // terminal flush-ack (the ack never comes while the writer is stalled).
    writer::__test_set_stall(true);

    let interp = Interpreter::new();
    let started = Instant::now();
    let value = interp.eval_str_compiled(
        r#"
        (def a
          (async/spawn
            (fn () (workflow/run "stalled" "doc" {} (fn () :done)))))
        (def b (async/spawn (fn () (async/sleep 20) :sibling-ran)))
        ;; Give A time to finish its body and park on the stalled flush-ack, and B to start.
        (async/sleep 60)
        (async/cancel a)
        (def bres (async/await b))
        (def ares (try (async/await a) (catch e :a-cancelled)))
        (list ares bres)
        "#,
    );
    let elapsed = started.elapsed();
    // Release the writer so its thread can drain + exit and the run dir can be cleaned up.
    writer::__test_disarm_stall();
    clear_env();

    let v = value.expect("program evaluated");
    let rendered = sema_core::pretty_print(&v, 100);
    assert!(
        rendered.contains(":sibling-ran"),
        "the sibling task must run to completion while A is parked on the stalled writer: {rendered}"
    );
    assert!(
        rendered.contains(":a-cancelled"),
        "cancelling the parked run must settle it (as a cancellation), not hang: {rendered}"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "cancelling a run parked on a stalled writer must settle PROMPTLY (no writer join); took {elapsed:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── (3) writer-side total-bytes odometer drops + marks (deterministic) ──

#[test]
fn journal_total_bytes_cap_drops_and_marks() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _stall = StallGuard;
    writer::__test_disarm_stall();
    clear_env();

    let dir = std::env::temp_dir().join(format!("sema-wfw-sizecap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_sizecap");
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &dir);
    // A tiny odometer: a handful of events blows past it deterministically (writer-side, no
    // timing race), so the writer records ONE marker and drops the rest.
    std::env::set_var("SEMA_WORKFLOW_JOURNAL_MAX_BYTES", "300");

    let interp = Interpreter::new();
    let value = interp.eval_str_compiled(
        r#"
        (workflow/run "sizecap" "doc" {}
          (fn ()
            (workflow/checkpoint :a (fn () 1))
            (workflow/checkpoint :b (fn () 2))
            (workflow/checkpoint :c (fn () 3))
            (workflow/checkpoint :d (fn () 4))
            (workflow/checkpoint :e (fn () 5))
            {:status :success}))
        "#,
    );
    clear_env();
    assert!(value.is_ok(), "workflow ran: {value:?}");

    let body = std::fs::read_to_string(dir.join("wf_sizecap").join("events.jsonl")).unwrap();
    assert!(
        body.contains(r#""event":"journal.overflow""#) && body.contains("journal-size-cap"),
        "the total-bytes odometer must record ONE journal.overflow marker: {body}"
    );
    // The run still finished (the flush barrier is honored even past the cap).
    assert!(dir.join("wf_sizecap").join("result.json").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

// ── (4) a full queue drops + marks WITHOUT blocking the enqueuing thread ──

#[test]
fn queue_full_drops_and_marks_without_blocking() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _stall = StallGuard;
    clear_env();

    let dir = std::env::temp_dir().join(format!("sema-wfw-qfull-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // A rendezvous-tight queue + a stalled writer means every enqueue after the first
    // overflows — deterministically, and (the point) WITHOUT blocking the caller.
    std::env::set_var("SEMA_WORKFLOW_JOURNAL_QUEUE", "1");
    writer::__test_set_stall(true);

    let journal = Journal::open(&dir, "wf_qfull").expect("open journal");
    let ev = |seq: u64| WorkflowEvent::RunEnded {
        seq,
        ts: "0".into(),
        status: "success".into(),
        reason: None,
        dur_ms: 0,
    };

    // Burst far past the queue bound while the writer is stalled: `try_send` never blocks,
    // so this returns fast even though all but the first message are dropped.
    let started = Instant::now();
    for i in 0..500 {
        journal.write(&ev(i));
    }
    let burst = started.elapsed();
    assert!(
        burst < Duration::from_secs(2),
        "enqueuing past a full queue must NOT block the VM thread; burst took {burst:?}"
    );

    // Release the writer, let it drain the one buffered message, then enqueue again: the
    // accumulated queue-full drops surface as ONE journal.overflow marker (space is now
    // free), which the writer records.
    writer::__test_disarm_stall();
    std::thread::sleep(Duration::from_millis(100));
    for i in 500..505 {
        journal.write(&ev(i));
    }
    journal.flush_blocking();
    drop(journal);
    std::thread::sleep(Duration::from_millis(50));

    let body = std::fs::read_to_string(dir.join("wf_qfull").join("events.jsonl")).unwrap();
    clear_env();
    assert!(
        body.contains(r#""event":"journal.overflow""#) && body.contains("queue-full"),
        "a queue-full overflow must surface exactly one journal.overflow (queue-full) marker: {body}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── (5) over-cap memo / rendered-value paths enforce all four caps ──

#[test]
fn over_cap_memo_and_value_paths_enforce_caps() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _stall = StallGuard;
    // Stall the writer so the many memo enqueues below never touch disk — the caps are all
    // enforced BEFORE the enqueue, in-memory, on the VM thread.
    writer::__test_set_stall(true);
    clear_env();

    // Caps 1+2 — MEMO_MAX_COUNT and MEMO_FILE_MAX_BYTES — exercised directly on a ctx.
    let ctx = WorkflowCtx::new("wf_caps".into(), Journal::null(), BTreeMap::new());

    // MEMO_FILE_MAX_BYTES: a value whose compact form exceeds the cap is NOT stored (and is
    // never JSON-encoded in full — the compact form is bounded-checked and aborts early).
    let oversized = Value::string(&"x".repeat(MEMO_FILE_MAX_BYTES + 128));
    ctx.memo_store("ck_oversized", &oversized);
    assert_eq!(
        ctx.memo_lookup("ck_oversized"),
        None,
        "an over-cap memo value must not be stored"
    );

    // MEMO_MAX_COUNT: the first cap-many small memos store; the next one is refused.
    for i in 0..MEMO_MAX_COUNT {
        ctx.memo_store(&format!("ck_{i}"), &Value::int(i as i64));
    }
    ctx.memo_store("ck_over_count", &Value::string("small"));
    assert_eq!(
        ctx.memo_lookup("ck_0"),
        Some(Value::int(0)),
        "under-cap memos are stored"
    );
    assert_eq!(
        ctx.memo_lookup("ck_over_count"),
        None,
        "a memo past MEMO_MAX_COUNT must not be stored"
    );
    drop(ctx);
    writer::__test_disarm_stall();

    // Cap 4 — rendered-value bytes — via a real run: a checkpoint of a large value renders
    // a TRUNCATED `value` field (the whole value is never materialized inline).
    let dir = std::env::temp_dir().join(format!("sema-wfw-render-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("SEMA_WORKFLOW_FIXED_TS", "0");
    std::env::set_var("SEMA_WORKFLOW_RUN_ID", "wf_render");
    std::env::set_var("SEMA_WORKFLOW_RUN_DIR", &dir);

    let interp = Interpreter::new();
    let value = interp.eval_str_compiled(
        r#"
        (workflow/run "render" "doc" {}
          (fn ()
            (workflow/checkpoint :big (fn () (range 8000)))
            {:status :success}))
        "#,
    );
    clear_env();
    assert!(value.is_ok(), "workflow ran: {value:?}");

    let events = read_events(&dir.join("wf_render").join("events.jsonl"));
    let checkpoint = events
        .iter()
        .find(|e| e["event"] == "checkpoint" && e["key"] == "big")
        .expect("checkpoint event present");
    let rendered = checkpoint["value"].as_str().unwrap_or("");
    assert!(
        rendered.contains("truncated at"),
        "a large checkpoint value must render truncated: {}",
        &rendered[..rendered.len().min(120)]
    );
    assert!(
        rendered.len() < 9000,
        "the rendered value must be byte-bounded (was {} bytes)",
        rendered.len()
    );

    let _ = std::fs::remove_dir_all(&dir);
}
