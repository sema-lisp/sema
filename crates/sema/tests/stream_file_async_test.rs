//! Async-offload coverage for file-backed `stream/*` (WP-STREAM).
//!
//! `crates/sema-stdlib/src/stream.rs` now branches on `in_async_context()`
//! AND whether the stream handed to it is file-backed (`stream_type()` is
//! `"file-input"`/`"file-output"`): `stream/open-input`/`stream/open-output`
//! offload the blocking `File::open`/`File::create` via `fs_offload` (mirrors
//! `db/open`); `stream/read`, `stream/write`, `stream/read-line`,
//! `stream/flush`, and `stream/close` offload the blocking op through a
//! CHECKOUT slot that lives directly on the stream object (no separate keyed
//! registry needed — the `Rc<StreamBox>` already IS the unique handle);
//! `stream/copy` checks out whichever side is file-backed when exactly one
//! side is (the other, a memory/stdio stream, is read/written on the VM
//! thread — fast, no I/O) and falls back to the unchanged synchronous loop
//! when BOTH sides are file-backed (a documented, narrow exception — see the
//! module doc comment in `stream.rs`). In-memory streams
//! (`stream/byte-buffer`, `stream/from-string`) never offload, even inside
//! async context — nothing to offload, they're pure CPU/memory. At top level
//! (no scheduler) every builtin keeps the original synchronous shape.
//!
//! Every file here is a small fresh temp file — no real disk latency needed
//! for these tests to be meaningful: the offload yields `AwaitIo` the instant
//! it's called (before the checkout's `spawn_blocking` closure has any
//! chance to run), so a zero-delay sibling task reliably completes first —
//! the same mechanism `db_async_test.rs`/`proc_pty_async_test.rs` rely on.
//! Ordering is asserted via channel receive order — never a wall-clock
//! duration assert.

#![cfg(not(target_arch = "wasm32"))]

use sema_core::Value;
use sema_eval::Interpreter;

/// A unique temp file path for one test, removed on drop (also on panic).
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("sema-stream-async-{tag}-{nanos}.txt"));
        TempFile(path)
    }
    fn with_contents(tag: &str, contents: &str) -> Self {
        let f = Self::new(tag);
        std::fs::write(&f.0, contents).unwrap();
        f
    }
    fn path(&self) -> String {
        self.0.to_string_lossy().to_string()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// === Scheduler-not-stalled: a sibling task completes while a stream/* op is in flight ===
//
// Pre-conversion, `stream/read-line` never yields, so the sibling (which
// sends immediately, no delay) can only run AFTER the whole read-line chain
// completes — "stream" always wins the channel race. Post-conversion each
// offloaded `stream/read-line` parks on `AwaitIo` the instant it's called,
// giving the scheduler a chance to run the sibling task first.
#[test]
fn stream_file_async_lets_sibling_run_first() {
    let f = TempFile::with_contents("sibling-order", "line1\nline2\nline3\n");
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (let ((s (stream/open-input "{path}")))
                  (stream/read-line s)
                  (stream/read-line s)
                  (stream/read-line s)
                  (stream/close s))
                (channel/send out "stream")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
        "#,
        path = f.path()
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("sibling-ordering program evaluated");
    let received: Vec<String> = result
        .as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(
        received,
        vec!["sibling".to_string(), "stream".to_string()],
        "sibling task must complete while the offloaded stream/read-line chain is in flight \
         (pre-conversion stream always wins), got {received:?}"
    );
}

/// `stream/open-input` + `stream/read-line` inside `async/spawn` returns the
/// identical lines as the synchronous path — also exercises `open_input`'s
/// `fs_offload` path (a distinct code path from the checkout-based ops).
#[test]
fn stream_file_async_read_line_matches_sync() {
    let f_sync = TempFile::with_contents("readline-sync", "a\nb\nc\n");
    let f_async = TempFile::with_contents("readline-async", "a\nb\nc\n");
    let interp = Interpreter::new();

    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"(let ((s (stream/open-input "{path}")))
                 (let ((r (list (stream/read-line s) (stream/read-line s)
                                 (stream/read-line s) (stream/read-line s))))
                   (stream/close s)
                   r))"#,
            path = f_sync.path()
        ))
        .expect("sync read-line chain");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn ()
                 (let ((s (stream/open-input "{path}")))
                   (let ((r (list (stream/read-line s) (stream/read-line s)
                                   (stream/read-line s) (stream/read-line s))))
                     (stream/close s)
                     r)))))"#,
            path = f_async.path()
        ))
        .expect("async read-line chain");
    assert_eq!(sync_v, async_v);
    assert_eq!(
        sync_v,
        Value::list(vec![
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
            Value::nil(),
        ])
    );
}

/// `stream/read` inside `async/spawn` returns identical bytes to sync.
#[test]
fn stream_file_async_read_matches_sync() {
    let f_sync = TempFile::with_contents("read-sync", "hello streams");
    let f_async = TempFile::with_contents("read-async", "hello streams");
    let interp = Interpreter::new();

    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"(let ((s (stream/open-input "{path}")))
                 (let ((d (utf8->string (stream/read-all s))))
                   (stream/close s)
                   d))"#,
            path = f_sync.path()
        ))
        .expect("sync read-all");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn ()
                 (let ((s (stream/open-input "{path}")))
                   (let ((d (utf8->string (stream/read s 5))))
                     (stream/close s)
                     d)))))"#,
            path = f_async.path()
        ))
        .expect("async stream/read");
    assert_eq!(sync_v, Value::string("hello streams"));
    assert_eq!(async_v, Value::string("hello"));
}

/// `stream/open-output` + `stream/write-string` + `stream/flush` +
/// `stream/close` inside `async/spawn` produces byte-identical file content
/// to the synchronous path.
#[test]
fn stream_file_async_write_flush_close_matches_sync() {
    let dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sync_path = dir.join(format!("sema-stream-async-write-sync-{nanos}.txt"));
    let async_path = dir.join(format!("sema-stream-async-write-async-{nanos}.txt"));

    let interp = Interpreter::new();
    interp
        .eval_str_compiled(&format!(
            r#"(let ((s (stream/open-output "{path}")))
                 (stream/write-string s "hello ")
                 (stream/write-string s "world")
                 (stream/flush s)
                 (stream/close s))"#,
            path = sync_path.display()
        ))
        .expect("sync write chain");
    interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn ()
                 (let ((s (stream/open-output "{path}")))
                   (stream/write-string s "hello ")
                   (stream/write-string s "world")
                   (stream/flush s)
                   (stream/close s)))))"#,
            path = async_path.display()
        ))
        .expect("async write chain");

    let sync_contents = std::fs::read_to_string(&sync_path).unwrap();
    let async_contents = std::fs::read_to_string(&async_path).unwrap();
    assert_eq!(sync_contents, "hello world");
    assert_eq!(async_contents, "hello world");

    let _ = std::fs::remove_file(&sync_path);
    let _ = std::fs::remove_file(&async_path);
}

/// `stream/copy` from a FILE-backed src into a `stream/byte-buffer` dst
/// (exercises `maybe_async_copy`'s "src is file" branch — the memory dst is
/// written on the VM thread, never offloaded).
#[test]
fn stream_file_async_copy_file_to_bytebuffer_matches_sync() {
    let f = TempFile::with_contents("copy-to-buf", "copy this content");
    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"(let ((src (stream/open-input "{path}"))
                     (dst (stream/byte-buffer)))
                 (let ((n (stream/copy src dst)))
                   (stream/close src)
                   (list n (utf8->string (stream/to-bytes dst)))))"#,
            path = f.path()
        ))
        .expect("sync copy file->buffer");
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn ()
                 (let ((src (stream/open-input "{path}"))
                       (dst (stream/byte-buffer)))
                   (let ((n (stream/copy src dst)))
                     (stream/close src)
                     (list n (utf8->string (stream/to-bytes dst))))))))"#,
            path = f.path()
        ))
        .expect("async copy file->buffer");
    assert_eq!(sync_v, async_v);
    assert_eq!(
        sync_v,
        Value::list(vec![Value::int(17), Value::string("copy this content")])
    );
}

/// `stream/copy` from a `stream/from-string` src into a FILE-backed dst
/// (exercises `maybe_async_copy`'s "dst is file" branch — src is read fully
/// on the VM thread first, then the write is offloaded).
#[test]
fn stream_file_async_copy_bytebuffer_to_file_matches_sync() {
    let dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dst_path = dir.join(format!("sema-stream-async-copy-to-file-{nanos}.txt"));

    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn ()
                 (let ((src (stream/from-string "from memory"))
                       (dst (stream/open-output "{path}")))
                   (let ((n (stream/copy src dst)))
                     (stream/close dst)
                     n)))))"#,
            path = dst_path.display()
        ))
        .expect("async copy buffer->file");
    assert_eq!(result, Value::int(11));
    assert_eq!(std::fs::read_to_string(&dst_path).unwrap(), "from memory");

    let _ = std::fs::remove_file(&dst_path);
}

/// `stream/copy` between two FILE-backed streams inside async context: per
/// the documented policy this deliberately falls back to the synchronous
/// loop rather than implementing dual-checkout — still correct, just not
/// yielding for this one call. Proves it still produces the right result.
#[test]
fn stream_file_async_copy_file_to_file_still_works() {
    let src = TempFile::with_contents("copy-ff-src", "file to file");
    let dst_path = {
        let dir = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        dir.join(format!("sema-stream-async-copy-ff-dst-{nanos}.txt"))
    };
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn ()
                 (let ((s (stream/open-input "{src}"))
                       (d (stream/open-output "{dst}")))
                   (let ((n (stream/copy s d)))
                     (stream/close s)
                     (stream/close d)
                     n)))))"#,
            src = src.path(),
            dst = dst_path.display()
        ))
        .expect("async file->file copy");
    assert_eq!(result, Value::int(12));
    assert_eq!(std::fs::read_to_string(&dst_path).unwrap(), "file to file");
    let _ = std::fs::remove_file(&dst_path);
}

/// Three sibling tasks all call `stream/read-line` on the SAME shared input
/// stream concurrently. Only one can hold the checkout at a time; the others
/// must queue (the `Acquire` phase re-attempting checkout each poll) rather
/// than deadlock, panic, or lose a line — proving the queued-caller path
/// documented in `stream.rs`. Which task wins which line isn't deterministic,
/// so the three results are compared as a sorted set.
#[test]
fn stream_file_async_queued_reads_on_one_stream_all_succeed() {
    let f = TempFile::with_contents("queued-reads", "l1\nl2\nl3\n");
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((s (stream/open-input "{path}"))
                  (out (channel/new 8)))
              (async/all
                (list
                  (async/spawn (fn () (channel/send out (stream/read-line s))))
                  (async/spawn (fn () (channel/send out (stream/read-line s))))
                  (async/spawn (fn () (channel/send out (stream/read-line s))))))
              (let ((r1 (channel/recv out))
                    (r2 (channel/recv out))
                    (r3 (channel/recv out)))
                (stream/close s)
                (sort (list r1 r2 r3))))
            "#,
            path = f.path()
        ))
        .expect("concurrent read-line on one shared stream");
    assert_eq!(
        result,
        Value::list(vec![
            Value::string("l1"),
            Value::string("l2"),
            Value::string("l3"),
        ]),
        "all three queued readers must complete and land distinct lines"
    );
}

/// In-memory streams (`stream/byte-buffer`, `stream/from-string`) never
/// offload — there's nothing to offload, they're pure CPU/memory — so they
/// must keep working correctly at top level AND inside async context on the
/// unchanged synchronous path.
#[test]
fn stream_file_async_memory_streams_stay_sync_in_async_context() {
    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled(
            r#"(let ((s (stream/byte-buffer)))
                 (stream/write s (string->utf8 "in-memory"))
                 (utf8->string (stream/to-bytes s)))"#,
        )
        .expect("sync byte-buffer roundtrip");
    let async_v = interp
        .eval_str_compiled(
            r#"(await (async/spawn (fn ()
                 (let ((s (stream/byte-buffer)))
                   (stream/write s (string->utf8 "in-memory"))
                   (utf8->string (stream/to-bytes s))))))"#,
        )
        .expect("async byte-buffer roundtrip");
    assert_eq!(sync_v, Value::string("in-memory"));
    assert_eq!(async_v, Value::string("in-memory"));
}

/// Reading a closed file stream inside async context errors the same way the
/// sync path does (`test_stream_read_closed_file` in `integration_test.rs`
/// pins the sync behavior this mirrors) — proving `maybe_async_read`'s
/// `is_closed()` guard fires instead of silently checking out a slot that's
/// still `Available` (closing an input stream never touches `FileInSlot`).
#[test]
fn stream_file_async_read_closed_file_errors() {
    let f = TempFile::with_contents("closed", "data");
    let interp = Interpreter::new();
    let err = interp
        .eval_str_compiled(&format!(
            r#"(await (async/spawn (fn ()
                 (let ((s (stream/open-input "{path}")))
                   (stream/close s)
                   (stream/read s 1)))))"#,
            path = f.path()
        ))
        .expect_err("reading a closed stream async must error");
    assert!(
        err.to_string().contains("stream/read: stream is closed")
            || err.to_string().contains("closed"),
        "expected a closed-stream error, got: {err}"
    );
}
