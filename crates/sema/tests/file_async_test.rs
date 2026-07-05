//! Cooperative-yield tests for the offloaded `file/*` builtins.
//!
//! Inside an `async/spawn`'d task the converted file ops (`file/read`,
//! `file/read-bytes`, `file/read-lines`, `file/write`, `file/append`,
//! `file/copy`, `file/delete`) offload their blocking `std::fs` work onto the
//! shared runtime and park on `AwaitIo`, so sibling tasks keep running while a
//! big read/write is in flight. At top level (no scheduler) they stay fully
//! synchronous.

mod common;

use common::eval;
use sema_core::Value;
use sema_eval::Interpreter;

/// Serializes the timing-sensitive tests (oracle, overlap, perf report) so
/// they don't contend with each other for the blocking pool and the disk while
/// measuring. `lock()` recovers from poisoning: a panicking timing test must
/// not cascade into the others.
static TIMING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn timing_guard() -> std::sync::MutexGuard<'static, ()> {
    TIMING_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A unique temp dir for one test, removed on drop (also on panic).
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sema-file-async-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().to_string()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write an `mb`-megabyte ASCII file (valid UTF-8, so `file/read` works on it).
fn write_big_file(path: &str, mb: usize) {
    std::fs::write(path, vec![b'x'; mb * 1024 * 1024]).unwrap();
}

// === THE ORACLE: a sleep-ticker sibling advances DURING a file workload ===
//
// Task A performs a slow file workload (several 64 MB reads + a copy); task B
// is a 1 ms sleep ticker bumping a shared counter. A snapshots the counter
// immediately before its workload and returns the delta after. If the file ops
// block the VM thread (the pre-conversion behavior), the ticker can never run
// between A's ops and the delta is 0. With the ops offloaded and parked on
// `AwaitIo`, the scheduler runs B while A's reads are in flight, so the delta
// is positive.
#[test]
fn oracle_ticker_advances_during_file_workload() {
    let _guard = timing_guard();
    let dir = TempDir::new("oracle");
    let big = dir.path("big.txt");
    let copy = dir.path("copy.txt");
    write_big_file(&big, 64);

    let program = format!(
        r#"
        (define ticks 0)
        (define ticker
          (async/spawn (fn ()
            (let loop ((i 0))
              (when (< i 5000)
                (async/sleep 1)
                (set! ticks (+ ticks 1))
                (loop (+ i 1)))))))
        (define worker
          (async/spawn (fn ()
            (let ((before ticks))
              (file/read "{big}")
              (file/read "{big}")
              (file/read "{big}")
              (file/copy "{big}" "{copy}")
              (file/read "{copy}")
              (- ticks before)))))
        (let ((delta (await worker)))
          (async/cancel ticker)
          delta)
        "#
    );

    let delta = eval(&program);
    let delta = delta.as_int().expect("worker should return an int delta");
    assert!(
        delta > 0,
        "sleep ticker must advance during the sibling's file workload \
         (got {delta} ticks; 0 means file I/O blocked the VM thread)"
    );
}

// === Correctness parity: async path returns the same values as the sync path ===

#[test]
fn async_write_read_roundtrip_parity() {
    let dir = TempDir::new("roundtrip");
    let sync_p = dir.path("sync.txt");
    let async_p = dir.path("async.txt");

    let sync_v = eval(&format!(
        r#"(begin (file/write "{sync_p}" "hello\nworld") (file/read "{sync_p}"))"#
    ));
    let async_v = eval(&format!(
        r#"(await (async/spawn (fn ()
             (file/write "{async_p}" "hello\nworld")
             (file/read "{async_p}"))))"#
    ));
    assert_eq!(sync_v, async_v);
    assert_eq!(async_v, Value::string("hello\nworld"));
    // Bytes on disk are identical too.
    assert_eq!(
        std::fs::read(&sync_p).unwrap(),
        std::fs::read(&async_p).unwrap()
    );
}

#[test]
fn async_read_bytes_and_read_lines_parity() {
    let dir = TempDir::new("bytes-lines");
    let p = dir.path("data.txt");
    std::fs::write(&p, "alpha\nbeta\ngamma").unwrap();

    let sync_bytes = eval(&format!(r#"(file/read-bytes "{p}")"#));
    let async_bytes = eval(&format!(
        r#"(await (async/spawn (fn () (file/read-bytes "{p}"))))"#
    ));
    assert_eq!(sync_bytes, async_bytes);

    let sync_lines = eval(&format!(r#"(file/read-lines "{p}")"#));
    let async_lines = eval(&format!(
        r#"(await (async/spawn (fn () (file/read-lines "{p}"))))"#
    ));
    assert_eq!(sync_lines, async_lines);
    assert_eq!(async_lines, eval(r#"'("alpha" "beta" "gamma")"#));
}

#[test]
fn async_append_copy_delete_parity() {
    let dir = TempDir::new("acd");
    let p = dir.path("log.txt");
    let q = dir.path("log-copy.txt");

    let v = eval(&format!(
        r#"(await (async/spawn (fn ()
             (file/write "{p}" "a")
             (file/append "{p}" "b")
             (file/append "{p}" "c")
             (file/copy "{p}" "{q}")
             (file/delete "{p}")
             (list (file/exists? "{p}") (file/read "{q}")))))"#
    ));
    assert_eq!(
        v,
        Value::list(vec![Value::bool(false), Value::string("abc")])
    );
}

// === Error parity: async rejections carry the sync path's exact IO message ===

#[test]
fn async_read_missing_file_error_matches_sync() {
    let dir = TempDir::new("missing");
    let p = dir.path("nope.txt");

    let interp = Interpreter::new();
    let sync_err = interp
        .eval_str_compiled(&format!(r#"(file/read "{p}")"#))
        .unwrap_err()
        .to_string();
    let async_err = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (file/read "{p}"))))"#
        ))
        .unwrap_err()
        .to_string();

    // The sync path errors with `IO error: file/read <path>: <os error>`; the
    // async rejection must carry that identical message (wrapped in the
    // standard `async/await: task rejected:` envelope every task error gets).
    assert!(
        sync_err.contains(&format!("file/read {p}: ")),
        "sync error shape changed: {sync_err}"
    );
    assert!(
        async_err.contains(&sync_err),
        "async rejection must embed the byte-identical sync IO message\n  sync:  {sync_err}\n  async: {async_err}"
    );
}

#[test]
fn async_delete_missing_file_error_matches_sync() {
    let dir = TempDir::new("del-missing");
    let p = dir.path("nope.txt");

    let interp = Interpreter::new();
    let sync_err = interp
        .eval_str_compiled(&format!(r#"(file/delete "{p}")"#))
        .unwrap_err()
        .to_string();
    let async_err = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (file/delete "{p}"))))"#
        ))
        .unwrap_err()
        .to_string();
    assert!(
        async_err.contains(&sync_err),
        "\n  sync:  {sync_err}\n  async: {async_err}"
    );
}

#[test]
fn async_copy_missing_src_error_matches_sync() {
    let dir = TempDir::new("copy-missing");
    let src = dir.path("nope.txt");
    let dst = dir.path("out.txt");

    let interp = Interpreter::new();
    let sync_err = interp
        .eval_str_compiled(&format!(r#"(file/copy "{src}" "{dst}")"#))
        .unwrap_err()
        .to_string();
    let async_err = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (file/copy "{src}" "{dst}"))))"#
        ))
        .unwrap_err()
        .to_string();
    assert!(
        sync_err.contains(&format!("file/copy {src} -> {dst}: ")),
        "sync error shape changed: {sync_err}"
    );
    assert!(
        async_err.contains(&sync_err),
        "\n  sync:  {sync_err}\n  async: {async_err}"
    );
}

// === Concurrency: two tasks doing big reads overlap ===
//
// Like-for-like comparison, both modes on the async (offload) path so the
// per-op fixed costs (scheduler round-trip, decode copy on the VM thread) are
// identical and the ONLY difference is whether the offloaded `std::fs` reads
// can be in flight at the same time: two tasks awaited one after the other
// (serialized) vs `async/all` (overlapping). Each task reads its own 64 MB
// file three times. Page-cache-hot reads are used (not writes) because their
// latency is memcpy-stable, where big writes are at the mercy of the
// filesystem's writeback daemon and flake. Asserted at 0.85 with margin.
#[test]
fn concurrent_big_reads_overlap() {
    let _guard = timing_guard();
    let dir = TempDir::new("overlap");
    let a = dir.path("a.txt");
    let b = dir.path("b.txt");
    write_big_file(&a, 64);
    write_big_file(&b, 64);

    let interp = Interpreter::new();
    let task =
        |p: &str| format!(r#"(fn () (file/read "{p}") (file/read "{p}") (file/read "{p}"))"#);

    // Warm-up: pull both files into the page cache on the async path.
    interp
        .eval_str_compiled(&format!(
            r#"(async/all (list (async/spawn {}) (async/spawn {})))"#,
            task(&a),
            task(&b)
        ))
        .expect("warmup reads");

    let t0 = std::time::Instant::now();
    interp
        .eval_str_compiled(&format!(
            r#"(begin
                 (await (async/spawn {}))
                 (await (async/spawn {})))"#,
            task(&a),
            task(&b)
        ))
        .expect("sequential awaited reads");
    let sequential = t0.elapsed();

    let t1 = std::time::Instant::now();
    interp
        .eval_str_compiled(&format!(
            r#"(async/all (list (async/spawn {}) (async/spawn {})))"#,
            task(&a),
            task(&b)
        ))
        .expect("concurrent reads");
    let concurrent = t1.elapsed();

    println!("big-read overlap: sequential={sequential:?} concurrent={concurrent:?}");
    assert!(
        concurrent.as_secs_f64() < sequential.as_secs_f64() * 0.85,
        "two tasks' offloaded big reads should overlap: concurrent={concurrent:?} sequential={sequential:?}"
    );
}

// === Sandbox: capability/path checks run on the VM thread BEFORE offload ===

#[test]
fn sandbox_fs_write_denied_in_async_context() {
    let dir = TempDir::new("sandbox-write");
    let p = dir.path("out.txt");
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let err = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (file/write "{p}" "hi"))))"#
        ))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Permission denied"),
        "fs-denied sandbox must reject file/write in async context: {err}"
    );
    assert!(!std::path::Path::new(&p).exists());
}

#[test]
fn sandbox_fs_read_denied_in_async_context() {
    let dir = TempDir::new("sandbox-read");
    let p = dir.path("secret.txt");
    std::fs::write(&p, "secret").unwrap();
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let err = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn () (file/read "{p}"))))"#
        ))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Permission denied"),
        "fs-denied sandbox must reject file/read in async context: {err}"
    );
}

// === Perf report: small-file async-path overhead (printed, not gated) ===
//
// 1000 × 1 KB `file/read` at top level (sync path) vs inside one async task
// (offload path). Prints the per-op numbers so the overhead is measured, not
// guessed. Run with `--nocapture` to see the report. No hard assertion beyond
// a sanity ceiling: offload cost is thread-handoff bound, not content bound.
#[test]
fn small_file_async_overhead_report() {
    let _guard = timing_guard();
    let dir = TempDir::new("perf");
    let p = dir.path("small.txt");
    std::fs::write(&p, vec![b'x'; 1024]).unwrap();

    let interp = Interpreter::new();
    let loop_src =
        format!(r#"(let loop ((i 0)) (when (< i 1000) (file/read "{p}") (loop (+ i 1))))"#);

    // Warm up the page cache and the interpreter.
    interp.eval_str_compiled(&loop_src).expect("warmup");

    let t0 = std::time::Instant::now();
    interp.eval_str_compiled(&loop_src).expect("sync loop");
    let sync_total = t0.elapsed();

    let async_src = format!(r#"(await (async/spawn (fn () {loop_src})))"#);
    let t1 = std::time::Instant::now();
    interp.eval_str_compiled(&async_src).expect("async loop");
    let async_total = t1.elapsed();

    let sync_us = sync_total.as_micros() as f64 / 1000.0;
    let async_us = async_total.as_micros() as f64 / 1000.0;
    println!(
        "small-file file/read (1 KB, 1000 ops): sync {sync_us:.1} us/op, \
         async {async_us:.1} us/op ({:.1}x)",
        async_us / sync_us
    );
}
