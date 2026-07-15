//! Canonical external-wait gates for the finite file operations (Task 05 R08A).
//!
//! Under the unified runtime (`eval_str_via_runtime`) the finite `file/*` ops
//! (`file/read`, `file/write`, `file/read-bytes`, `file/write-bytes`,
//! `file/exists?`, `file/list`, `file/info`) route through the CANONICAL
//! `WaitKind::External` path — a `PreparedExternalOperation` submitted to the
//! real thread-pool executor — NOT the legacy `LegacyAwaitIo` bridge. Each is
//! classified `QuarantinedBounded`: a hard byte/entry cap fixed on the VM thread
//! BEFORE dispatch, the job carries only an owned `Send` snapshot, computes
//! off-thread, and the result is decoded on the VM thread.
//!
//! These gates prove: (1) a write→read round-trip matches the synchronous
//! oracle; (2) two spawned reads run off-thread CONCURRENTLY (peak in-flight
//! >= 2); (3) the byte/entry cap is enforced before dispatch (oversized →
//! condition, never dispatched); (4) cancelling a spawned file op settles it
//! Cancelled without hanging (the quarantined job is detached and reaped).
//!
//! The cap/delay/in-flight gauges are process-global, so every test is
//! `#[serial]`.

#![cfg(not(target_arch = "wasm32"))]

use sema_eval::Interpreter;
use sema_stdlib::{
    fs_peak_inflight, reset_fs_inflight, set_fs_byte_cap, set_fs_list_cap, set_fs_test_delay_ms,
    FS_BYTE_CAP_DEFAULT, FS_LIST_CAP_DEFAULT,
};
use serial_test::serial;

/// A unique temp dir for one test, removed on drop (also on panic).
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sema-file-runtime-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self, name: &str) -> String {
        // Forward slashes keep the path a clean Sema string literal on every OS.
        self.0.join(name).to_string_lossy().replace('\\', "/")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        set_fs_test_delay_ms(0);
        set_fs_byte_cap(FS_BYTE_CAP_DEFAULT);
        set_fs_list_cap(FS_LIST_CAP_DEFAULT);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Gate 1: `file/write` then `file/read` round-trips through the runtime and
/// matches the synchronous `eval_str` oracle.
#[test]
#[serial]
fn write_then_read_round_trips_through_runtime() {
    set_fs_test_delay_ms(0);
    let dir = TempDir::new("roundtrip");
    let path = dir.path("hello.txt");
    let interp = Interpreter::new();

    let program =
        format!(r#"(begin (file/write "{path}" "canonical external wait") (file/read "{path}"))"#);
    let runtime_result = interp
        .eval_str_via_runtime(&program)
        .expect("write+read round-trips through the runtime");
    assert_eq!(runtime_result.as_str(), Some("canonical external wait"));

    // The synchronous oracle produces the identical value.
    let oracle = interp
        .eval_str(&format!(r#"(file/read "{path}")"#))
        .expect("synchronous oracle read");
    assert_eq!(
        runtime_result, oracle,
        "runtime read matches the sync oracle"
    );
}

/// Gate 2: two `async/spawn`ed `file/read`s of different files run their jobs
/// OFF the VM thread SIMULTANEOUSLY on separate workers — proven by peak
/// in-flight >= 2 (a per-job delay holds each slot so the overlap is
/// deterministic, not a timing race).
#[test]
#[serial]
fn two_spawned_reads_overlap_off_thread() {
    let dir = TempDir::new("overlap");
    let a = dir.path("a.txt");
    let b = dir.path("b.txt");
    std::fs::write(dir.0.join("a.txt"), "alpha").unwrap();
    std::fs::write(dir.0.join("b.txt"), "beta").unwrap();

    reset_fs_inflight();
    set_fs_test_delay_ms(200);

    let interp = Interpreter::new();
    let program = format!(
        r#"
        (let ((x (async/spawn (fn () (file/read "{a}"))))
              (y (async/spawn (fn () (file/read "{b}")))))
          (async/all (list x y)))
        "#
    );
    let result = interp
        .eval_str_via_runtime(&program)
        .expect("two spawned reads overlap through the runtime");
    let items = result.as_list().expect("(results)");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].as_str(), Some("alpha"));
    assert_eq!(items[1].as_str(), Some("beta"));

    assert!(
        fs_peak_inflight() >= 2,
        "expected peak in-flight >= 2 (true off-thread overlap), got {}",
        fs_peak_inflight()
    );
}

/// Gate 3 (resource contract — cap): an oversized `file/read` is rejected on the
/// VM thread BEFORE dispatch — a Sema condition, never an unbounded allocation,
/// and the job is NEVER submitted to a worker (peak in-flight stays 0). This is
/// the `QuarantinedBounded` pre-dispatch byte-cap contract.
#[test]
#[serial]
fn oversized_read_is_rejected_before_dispatch() {
    let dir = TempDir::new("cap");
    let path = dir.path("big.txt");
    std::fs::write(dir.0.join("big.txt"), vec![b'x'; 4096]).unwrap();

    reset_fs_inflight();
    set_fs_byte_cap(16); // a 4096-byte file is far over the 16-byte cap

    let interp = Interpreter::new();
    let err = interp
        .eval_str_via_runtime(&format!(r#"(file/read "{path}")"#))
        .expect_err("an oversized read is rejected as a condition");
    let message = err.to_string();
    assert!(
        message.contains("exceeds") && message.contains("cap"),
        "expected a byte-cap condition, got: {message}"
    );
    assert_eq!(
        fs_peak_inflight(),
        0,
        "an over-cap read must never dispatch a worker job"
    );
}

/// Gate 3 (resource contract — entry cap): a `file/list` whose directory exceeds
/// the fixed entry cap aborts with a named bound-exceeded condition rather than
/// allocating an unbounded entry list.
#[test]
#[serial]
fn oversized_list_is_bounded() {
    let dir = TempDir::new("listcap");
    for i in 0..8 {
        std::fs::write(dir.0.join(format!("f{i}.txt")), "x").unwrap();
    }
    let path = dir.path("");
    set_fs_test_delay_ms(0);
    set_fs_list_cap(2); // 8 entries > 2-entry cap

    let interp = Interpreter::new();
    let err = interp
        .eval_str_via_runtime(&format!(r#"(file/list "{path}")"#))
        .expect_err("an over-cap list aborts with a bound-exceeded condition");
    let message = err.to_string();
    assert!(
        message.contains("cap") && message.contains("entry"),
        "expected an entry-cap condition, got: {message}"
    );
}

/// Gate 4 (resource contract — cancellation class): `async/cancel` on a spawned
/// `file/read` that is PARKED on its external wait settles the task Cancelled.
/// The quarantined job is bounded (detached and reaped by its late completion),
/// so a cancelled file read never hangs the drive or the drop.
#[test]
#[serial]
fn cancel_of_spawned_file_read_settles_cancelled() {
    let dir = TempDir::new("cancel");
    let path = dir.path("slow.txt");
    std::fs::write(dir.0.join("slow.txt"), "kept parked by the per-job delay").unwrap();

    // A per-job delay guarantees the spawned read is still parked on its external
    // wait when the cancellation lands.
    set_fs_test_delay_ms(400);

    let interp = Interpreter::new();
    let program = format!(
        r#"
        (let ((p (async/spawn (fn () (file/read "{path}")))))
          (async/sleep 20)
          (async/cancel p)
          (try (await p) (catch e (:type e))))
        "#
    );
    let result = interp
        .eval_str_via_runtime(&program)
        .expect("cancel of a parked file read drives through the runtime");
    assert_eq!(
        result.as_keyword().as_deref(),
        Some("cancelled"),
        "awaiting a cancelled parked file read raises the :cancelled condition"
    );
}
