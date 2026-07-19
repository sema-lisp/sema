//! Async-offload coverage for file-backed `stream/*` (WP-STREAM).
//!
//! `crates/sema-stdlib/src/stream.rs` branches on `in_runtime_quantum()` and
//! whether the stream handed to it is file-backed (`stream_type()` is
//! `"file-input"`/`"file-output"`): `stream/open-input`/`stream/open-output`
//! offload the blocking `File::open`/`File::create`; `stream/read`,
//! `stream/write`, `stream/read-line`,
//! `stream/flush`, and `stream/close` offload the blocking op through a
//! CHECKOUT slot that lives directly on the stream object (no separate keyed
//! registry needed — the `Rc<StreamBox>` already IS the unique handle);
//! `stream/copy` checks out whichever side is file-backed when exactly one
//! side is (the other, a memory stream, is read/written on the VM thread —
//! fast, no I/O); a runtime-quantum file-to-file copy fails promptly with
//! bounded-chunk guidance instead of entering a VM-thread EOF loop. In-memory
//! streams (`stream/byte-buffer`, `stream/from-string`) never offload because
//! they are pure CPU/memory. The direct value ABI keeps its bounded synchronous
//! compatibility path.
//!
//! Every file here is a small fresh temp file — no real disk latency needed
//! for these tests to be meaningful: the offload parks on an External wait the
//! instant it is called (before the checkout's blocking closure has any
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

fn eval_with_stdin(program: &str, input: &[u8]) -> String {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", program])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema with piped stdin");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write complete stdin fixture");
    let output = child.wait_with_output().expect("collect sema output");
    assert!(
        output.status.success(),
        "sema exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("sema output is UTF-8")
        .trim()
        .to_string()
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

#[test]
fn used_file_streams_dropped_without_close_release_their_runtime_gates() {
    let input = TempFile::with_contents("drop-input-gate", "hello\n");
    let output = TempFile::new("drop-output-gate");
    let interp = Interpreter::new();

    interp
        .eval_str_via_runtime(&format!(
            r#"(let ((s (stream/open-input "{}")))
                  (stream/read-line s)
                  nil)"#,
            input.path()
        ))
        .expect("used input stream drops at the end of the runtime root");
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "dropping a used file-input stream must close its owner runtime gate"
    );

    interp
        .eval_str_via_runtime(&format!(
            r#"(let ((s (stream/open-output "{}")))
                  (stream/write-string s "hello")
                  nil)"#,
            output.path()
        ))
        .expect("used output stream drops at the end of the runtime root");
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "dropping a used file-output stream must close its owner runtime gate"
    );
}

#[test]
fn file_input_close_from_foreign_runtime_closes_the_owner_gate() {
    let input = TempFile::with_contents("foreign-close-input", "hello\n");
    let owner = Interpreter::new();
    let caller = Interpreter::new();
    let stream = owner
        .eval_str_via_runtime(&format!(
            r#"(define foreign-input (stream/open-input "{}"))
                (stream/read-line foreign-input)
                foreign-input"#,
            input.path()
        ))
        .expect("owner runtime creates and uses file-input stream");
    caller.global_env.set_str("foreign-input", stream);
    assert_eq!(owner.runtime_resource_gate_count(), 1);
    assert_eq!(caller.runtime_resource_gate_count(), 0);

    let result = caller
        .eval_str_via_runtime("(stream/close foreign-input)")
        .expect("foreign runtime routes file-input close through the gate owner");
    assert!(result.is_nil());
    assert_eq!(owner.runtime_resource_gate_count(), 0);
    assert_eq!(caller.runtime_resource_gate_count(), 0);
}

#[test]
fn buffered_file_output_close_from_foreign_runtime_flushes_and_closes_owner_gate() {
    let output = TempFile::new("foreign-close-output");
    let owner = Interpreter::new();
    let caller = Interpreter::new();
    let stream = owner
        .eval_str_via_runtime(&format!(
            r#"(define foreign-output (stream/open-output "{}"))
                (stream/write-string foreign-output "buffered output")
                foreign-output"#,
            output.path()
        ))
        .expect("owner runtime creates and writes buffered file-output stream");
    caller.global_env.set_str("foreign-output", stream);
    assert_eq!(owner.runtime_resource_gate_count(), 1);
    assert_eq!(caller.runtime_resource_gate_count(), 0);

    let result = caller
        .eval_str_via_runtime("(stream/close foreign-output)")
        .expect("foreign runtime offloads buffered output teardown without the owner gate");
    assert!(result.is_nil());
    assert_eq!(owner.runtime_resource_gate_count(), 0);
    assert_eq!(caller.runtime_resource_gate_count(), 0);
    assert_eq!(
        std::fs::read_to_string(&output.0).expect("read flushed foreign output"),
        "buffered output"
    );
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
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "stream/close must return both file stream gates to baseline"
    );

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

#[test]
fn stream_aggregation_caps_accept_the_boundary_and_reject_one_byte_over() {
    let interp = Interpreter::new();

    let exact = interp
        .eval_str_compiled(r#"(utf8->string (stream/read-all (stream/from-string "12345678") 8))"#)
        .expect("read-all accepts exactly max-bytes");
    assert_eq!(exact, Value::string("12345678"));

    let read_err = interp
        .eval_str_compiled(r#"(stream/read-all (stream/from-string "123456789") 8)"#)
        .expect_err("read-all rejects one byte beyond max-bytes");
    assert!(
        read_err.to_string().contains("8-byte cap"),
        "expected the configured byte cap in the error, got: {read_err}"
    );

    interp
        .eval_str_compiled(
            r#"(define capped-copy-src (stream/from-string "123456789"))
                (define capped-copy-dst (stream/byte-buffer))"#,
        )
        .expect("define copy streams");
    let copy_err = interp
        .eval_str_compiled("(stream/copy capped-copy-src capped-copy-dst 8)")
        .expect_err("copy rejects one byte beyond max-bytes");
    assert!(
        copy_err.to_string().contains("8-byte cap"),
        "expected the configured byte cap in the error, got: {copy_err}"
    );
    let copied = interp
        .eval_str_compiled("(stream/to-bytes capped-copy-dst)")
        .expect("inspect rejected copy destination");
    assert_eq!(
        copied.as_bytevector(),
        Some(&[][..]),
        "the over-cap chunk must be rejected before any destination write"
    );

    let exact_copy = interp
        .eval_str_compiled(
            r#"(let ((src (stream/from-string "12345678"))
                      (dst (stream/byte-buffer)))
                  (list (stream/copy src dst 8) (utf8->string (stream/to-bytes dst))))"#,
        )
        .expect("copy accepts exactly max-bytes");
    assert_eq!(
        exact_copy,
        Value::list(vec![Value::int(8), Value::string("12345678")])
    );

    interp
        .eval_str_compiled(
            r#"(define multi-copy-src
                   (stream/from-string (string/repeat "x" 16385)))
                (define multi-copy-dst (stream/byte-buffer))"#,
        )
        .expect("define multi-chunk copy streams");
    let multi_copy_err = interp
        .eval_str_compiled("(stream/copy multi-copy-src multi-copy-dst 16384)")
        .expect_err("copy rejects the overflow witness after multiple chunks");
    assert!(multi_copy_err.to_string().contains("16384-byte cap"));
    let copied_prefix = interp
        .eval_str_compiled("(stream/to-bytes multi-copy-dst)")
        .expect("inspect multi-chunk rejected destination");
    assert_eq!(
        copied_prefix.as_bytevector().map(<[u8]>::len),
        Some(16384),
        "the one-byte overflow witness must be rejected before its destination write"
    );
}

#[test]
fn stdin_operations_share_buffered_bytes_sequentially_and_concurrently() {
    let sequential_read = eval_with_stdin(
        r#"(list
              (utf8->string (stream/read *stdin* 1))
              (utf8->string (stream/read-all *stdin* 16)))"#,
        b"abc",
    );
    assert_eq!(sequential_read, r#"("a" "bc")"#);

    let sequential_line = eval_with_stdin(
        r#"(list
              (stream/read-line *stdin*)
              (utf8->string (stream/read-all *stdin* 16)))"#,
        b"a\nbc",
    );
    assert_eq!(sequential_line, r#"("a" "bc")"#);

    let sequential_copy = eval_with_stdin(
        r#"(let ((dst (stream/byte-buffer)))
              (list
                (utf8->string (stream/read *stdin* 1))
                (stream/copy *stdin* dst 16)
                (utf8->string (stream/to-bytes dst))))"#,
        b"abc",
    );
    assert_eq!(sequential_copy, r#"("a" 2 "bc")"#);

    let concurrent = eval_with_stdin(
        r#"(let ((first
                   (async/spawn
                     (fn () (utf8->string (stream/read *stdin* 3)))))
                  (rest
                   (async/spawn
                     (fn () (utf8->string (stream/read-all *stdin* 16))))))
              (async/all (list first rest)))"#,
        b"abcdef",
    );
    assert_eq!(
        concurrent, r#"("abc" "def")"#,
        "stdin operations must acquire FIFO ownership and consume disjoint bytes"
    );
}

#[test]
fn file_read_all_cap_releases_its_gate_after_success_and_rejection() {
    let exact_file = TempFile::with_contents("read-all-cap-exact", "12345678");
    let over_file = TempFile::with_contents("read-all-cap-over", "123456789");
    let interp = Interpreter::new();

    let exact = interp
        .eval_str_compiled(&format!(
            r#"(with-stream (s (stream/open-input "{}"))
                  (utf8->string (stream/read-all s 8)))"#,
            exact_file.path()
        ))
        .expect("file read-all accepts exactly max-bytes");
    assert_eq!(exact, Value::string("12345678"));
    assert_eq!(interp.runtime_resource_gate_count(), 0);

    let err = interp
        .eval_str_compiled(&format!(
            r#"(with-stream (s (stream/open-input "{}"))
                  (stream/read-all s 8))"#,
            over_file.path()
        ))
        .expect_err("file read-all rejects one byte beyond max-bytes");
    assert!(err.to_string().contains("8-byte cap"));
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "over-cap read-all cleanup returns the file gate to baseline"
    );
}

#[test]
fn stream_aggregation_value_abi_keeps_synchronous_compatibility() {
    let interp = Interpreter::new();
    let source = interp
        .eval_str_compiled(r#"(stream/from-string "sync-path")"#)
        .expect("construct source stream");
    let read_all = interp
        .global_env
        .get(sema_core::intern("stream/read-all"))
        .expect("stream/read-all builtin")
        .as_native_fn_rc()
        .expect("stream/read-all is native");

    let value = (read_all.func)(&interp.ctx, &[source, Value::int(9)])
        .expect("value ABI reads synchronously outside a runtime quantum");
    assert_eq!(value.as_bytevector(), Some(&b"sync-path"[..]));

    let source = interp
        .eval_str_compiled(r#"(stream/from-string "copy-sync")"#)
        .expect("construct copy source");
    let destination = interp
        .eval_str_compiled("(stream/byte-buffer)")
        .expect("construct copy destination");
    let copy = interp
        .global_env
        .get(sema_core::intern("stream/copy"))
        .expect("stream/copy builtin")
        .as_native_fn_rc()
        .expect("stream/copy is native");
    let copied = (copy.func)(&interp.ctx, &[source, destination.clone(), Value::int(9)])
        .expect("copy value ABI runs synchronously outside a runtime quantum");
    assert_eq!(copied, Value::int(9));
    let to_bytes = interp
        .global_env
        .get(sema_core::intern("stream/to-bytes"))
        .expect("stream/to-bytes builtin")
        .as_native_fn_rc()
        .expect("stream/to-bytes is native");
    let copied_bytes = (to_bytes.func)(&interp.ctx, std::slice::from_ref(&destination))
        .expect("inspect value-ABI copy destination");
    assert_eq!(
        copied_bytes.as_bytevector(),
        Some(&b"copy-sync"[..]),
        "the synchronous copy compatibility path writes the complete payload"
    );

    let file_source = TempFile::with_contents("value-abi-copy-src", "file-sync");
    let file_destination = TempFile::new("value-abi-copy-dst");
    let source = interp
        .eval_str_compiled(&format!(r#"(stream/open-input "{}")"#, file_source.path()))
        .expect("open value-ABI file source");
    let destination = interp
        .eval_str_compiled(&format!(
            r#"(stream/open-output "{}")"#,
            file_destination.path()
        ))
        .expect("open value-ABI file destination");
    let copied = (copy.func)(
        &interp.ctx,
        &[source.clone(), destination.clone(), Value::int(9)],
    )
    .expect("value ABI retains bounded synchronous file-to-file copy");
    assert_eq!(copied, Value::int(9));
    let close = interp
        .global_env
        .get(sema_core::intern("stream/close"))
        .expect("stream/close builtin")
        .as_native_fn_rc()
        .expect("stream/close is native");
    (close.func)(&interp.ctx, &[source]).expect("close value-ABI file source");
    (close.func)(&interp.ctx, &[destination]).expect("flush value-ABI file destination");
    assert_eq!(
        std::fs::read_to_string(&file_destination.0).expect("read copied file"),
        "file-sync"
    );
}

/// A runtime-quantum file-to-file copy must never enter the VM-thread EOF
/// loop. Until ordered dual-resource acquisition exists, it fails promptly
/// with bounded-chunk guidance and leaves both resource gates reclaimable.
#[test]
fn stream_file_async_copy_file_to_file_fails_fast_with_chunk_guidance() {
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
    let error = interp
        .eval_str_compiled(&format!(
            r#"(with-stream (s (stream/open-input "{src}"))
                  (with-stream (d (stream/open-output "{dst}"))
                    (stream/copy s d 1024)))"#,
            src = src.path(),
            dst = dst_path.display()
        ))
        .expect_err("runtime file->file copy must fail instead of blocking the VM thread");
    let message = error.to_string();
    assert!(
        message.contains("file-to-file") && message.contains("bounded chunks"),
        "expected actionable bounded-chunk guidance, got: {message}"
    );
    assert_eq!(interp.runtime_resource_gate_count(), 0);
    let _ = std::fs::remove_file(&dst_path);
}

fn assert_open_stdin_operations_are_cancellable(program: &str, expected_cancellations: usize) {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", program])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema with an open stdin pipe");

    // Deliberately keep the writer open and empty. The coordinated owner may
    // wait in its dedicated reader, but the logical operations remain
    // cancellable and let the sibling/root settle without a runtime worker.
    let open_stdin = child.stdin.take().expect("piped stdin");
    // Process startup can contend with other nextest cases; the old pinned
    // worker survives indefinitely while this cooperative path exits promptly.
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll sema child") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            drop(open_stdin);
            let output = child.wait_with_output().expect("reap timed-out sema child");
            panic!(
                "cancelled stdin aggregation left work pinned; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(open_stdin);
    let output = child.wait_with_output().expect("collect sema output");
    assert!(
        status.success(),
        "sema exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(":sibling")
            && stdout.matches(":cancelled").count() == expected_cancellations,
        "sibling must progress and stdin task must settle cancelled, got: {stdout}"
    );
}

#[test]
fn cancelled_open_stdin_aggregations_exit_without_pinned_workers_or_gates() {
    let destination = TempFile::new("stdin-copy-cancel");
    assert_open_stdin_operations_are_cancellable(
        &format!(
            r#"
        (let ((events (channel/new 2))
              (read-pending (async/spawn (fn () (stream/read-all *stdin* 64))))
              (copy-pending (async/spawn (fn ()
                (with-stream (dst (stream/open-output "{path}"))
                  (stream/copy *stdin* dst 64))))))
          (async/spawn (fn () (channel/send events :sibling)))
          (async/sleep 20)
          (async/cancel read-pending)
          (async/cancel copy-pending)
          (list (channel/recv events)
                (try (await read-pending) (catch e (:type e)))
                (try (await copy-pending) (catch e (:type e)))))
        "#,
            path = destination.path()
        ),
        2,
    );
    assert_eq!(
        std::fs::metadata(&destination.0)
            .expect("cancelled copy created its destination")
            .len(),
        0,
        "no stdin byte was written before the open-source cancellation"
    );
}

#[test]
fn cancelled_stdin_owner_releases_its_lease_and_preserves_inflight_bytes() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    let program = r#"
        (let ((pending (async/spawn (fn () (stream/read-all *stdin* 64)))))
          (async/sleep 20)
          (async/cancel pending)
          (try (await pending) (catch e nil))
          (stream/write-string *stdout* "lease-released\n")
          (stream/flush *stdout*)
          (utf8->string (stream/read *stdin* 1)))
    "#;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", program])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sema with piped stdin");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let (send_marker, receive_marker) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        let mut marker = String::new();
        let result = stdout.read_line(&mut marker).map(|_| (marker, stdout));
        let _ = send_marker.send(result);
    });

    let (marker, mut stdout) = match receive_marker.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(marker)) => marker,
        Ok(Err(error)) => panic!("read lease-release marker: {error}"),
        Err(error) => {
            let _ = child.kill();
            panic!("cancelled stdin owner did not release its lease: {error}");
        }
    };
    assert_eq!(marker, "lease-released\n");

    // The owner's OS read was already in flight for the cancelled aggregate.
    // Its byte must stay in the owner buffer and satisfy the next FIFO reader.
    stdin.write_all(b"z").expect("write post-cancel byte");
    drop(stdin);
    let status = child.wait().expect("wait for sema child");
    let mut remaining_stdout = String::new();
    stdout
        .read_to_string(&mut remaining_stdout)
        .expect("read post-marker stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr)
        .expect("read child stderr");
    assert!(status.success(), "sema exited non-zero: {stderr}");
    assert_eq!(remaining_stdout.trim(), r#""z""#);
}

#[test]
fn file_read_all_suspends_so_a_sibling_progresses_and_cleanup_reaches_baseline() {
    let input = TempFile::with_contents("read-all-sibling", &"x".repeat(32 * 1024));
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((stream (stream/open-input "{path}"))
                  (events (channel/new 2)))
              (async/all (list
                (async/spawn (fn ()
                  (stream/read-all stream 32768)
                  (channel/send events :aggregate)))
                (async/spawn (fn () (channel/send events :sibling)))))
              (let ((result (list (channel/recv events) (channel/recv events))))
                (stream/close stream)
                result))
            "#,
            path = input.path()
        ))
        .expect("read-all and sibling settle");
    assert_eq!(
        result,
        Value::list(vec![Value::keyword("sibling"), Value::keyword("aggregate")])
    );
    assert_eq!(interp.runtime_live_task_count(), 0);
    assert_eq!(interp.runtime_resource_gate_count(), 0);
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

/// Regression (VM closure/scheduler): a `file/for-each-line` / `file/fold-lines`
/// callback that CAPTURES a lexical upvalue must read the correct value when the
/// reader runs in async context — even alongside a NESTED `async/all` on a
/// sibling task. Two bugs made this fail before:
///   1. the per-line callback runs from a deferred I/O poll AFTER the owning task
///      suspends, so its still-`Open` upvalue read `nil` — fixed by snapshotting
///      the callback's upvalues (`snapshot_escaping_closure`) up front, while the
///      owning task VM is still current (exactly as `async/spawn` does);
///   2. routing that call through the inline-task path `take_scheduler()`'d while
///      the scheduler was already borrowed to drive the poll (reproducible under a
///      nested `async/all`) — fixed by running the callback synchronously on a
///      foreign VM (`run_closure_foreign_sync`), which never touches the scheduler.
#[test]
fn streaming_line_callback_captures_upvalue_under_nested_async() {
    let f = TempFile::with_contents("upvalue-stream", "a\nb\nc\nd\ne\n");
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (defun count-lines ()            ; for-each-line with int + mutable-cell upvalues
          (async
            (let ((seen (mutable-cell/new 0))
                  (base 100))
              (file/for-each-line "{path}"
                (fn (l) (mutable-cell/set! seen (+ (mutable-cell/get seen) base))))
              (mutable-cell/get seen))))
        (defun join-lines ()             ; fold-lines with a captured string upvalue
          (async
            (let ((tag "L:"))
              (file/fold-lines "{path}" (fn (acc l) (str acc tag l)) "start"))))
        (defun busy ()                   ; sibling task running a NESTED async/all
          (async (async/all (map (fn (i) (async (async/sleep 3) i)) (range 1 5)))))
        (async/all (list (count-lines) (join-lines) (busy)))
        "#,
        path = f.path()
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("upvalue-under-nested-async program evaluated");
    let items = result.as_list().expect("async/all returns a list");
    assert_eq!(
        items[0].as_int(),
        Some(500),
        "for-each-line callback read its captured `base` upvalue (5 lines * 100)"
    );
    assert_eq!(
        items[1].as_str(),
        Some("startL:aL:bL:cL:dL:e"),
        "fold-lines callback read its captured `tag` upvalue"
    );
}

/// Regression (VM transitive upvalue snapshot): a closure capturing a lexical
/// upvalue, passed as DATA into an async task, must read that upvalue when it is
/// finally invoked on the task's (foreign) VM. The task-closure snapshot didn't
/// recurse into closures reachable *through* its captured values, so the inner
/// closure's still-`Open` upvalue dereferenced a stack slot not on the task VM
/// ("captured variable's stack slot is not on this VM"). Fixed by recursing in
/// `close_closure_upvalues_for_foreign_run`.
#[test]
fn closure_with_upvalue_passed_as_data_into_async_task_survives() {
    let interp = Interpreter::new();
    let program = r#"
        (defun run-thunk (f) (async (f)))          ; wraps a PASSED closure in a task
        (let ((tmp "SCRATCH") (n 41))
          (async/all (list
            (run-thunk (fn () tmp))                ; captured string upvalue
            (run-thunk (fn () (+ n 1))))))         ; captured int upvalue
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("program evaluated");
    let items = result.as_list().expect("async/all list");
    assert_eq!(
        items[0].as_str(),
        Some("SCRATCH"),
        "string upvalue survived escape into task"
    );
    assert_eq!(
        items[1].as_int(),
        Some(42),
        "int upvalue survived escape into task"
    );
}

/// Regression: a CYCLIC closure graph (a→b→n, passed into a task) must snapshot
/// transitively AND terminate — each visited cell becomes `Tracked`, so the
/// back-edge finds nothing to do (no infinite recursion).
#[test]
fn cyclic_closures_passed_into_async_task_terminate() {
    let interp = Interpreter::new();
    let program = r#"
        (defun run-thunk (f) (async (f)))
        (let ((n 7))
          (letrec ((a (fn () (if (> n 0) (str "a" (b)) "")))
                   (b (fn () (str "b" n))))
            (async/all (list (run-thunk a)))))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("cyclic-closure program evaluated");
    assert_eq!(result.as_list().unwrap()[0].as_str(), Some("ab7"));
}

// === Cancellation through the ResourceGate + checkout_external path ===
//
// Cancelling spawned file-stream ops must settle the tasks (never hang or panic).
// A queued reader cancelled while parked behind the gate holder settles
// :cancelled; the gate holder, cancelled while in flight, tombstones ITS stream
// (best-effort — the resource is stuck in the blocking worker, matching the
// documented policy). The runtime itself is not wedged: a FRESHLY opened stream
// afterward reads normally. Exercises the checkout continuations' Cancelled arms
// + the per-stream ResourceGate release.
#[test]
fn stream_file_async_cancel_settles_and_fresh_stream_works() {
    let f = TempFile::with_contents("cancel-queued", "a\nb\nc\nd\n");
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((s (stream/open-input "{path}")))
              (let ((slow (async/spawn (fn () (stream/read-line s))))
                    (queued (async/spawn (fn () (stream/read-line s)))))
                (async/cancel queued)
                (async/cancel slow)
                (let ((q (try (async/await queued) (catch e :q-cancelled)))
                      (sl (try (async/await slow) (catch e :slow-settled))))
                  ;; the runtime is not wedged: a FRESH stream reads normally
                  (let ((s2 (stream/open-input "{path}")))
                    (let ((line (stream/read-line s2)))
                      (stream/close s2)
                      (list q sl line))))))
            "#,
            path = f.path()
        ))
        .expect("cancelled stream chain evaluates without wedging the runtime");
    let parts: Vec<Value> = result.as_list().expect("result list").to_vec();
    assert_eq!(
        parts[0],
        Value::keyword("q-cancelled"),
        "the queued-then-cancelled reader must settle :cancelled, got {:?}",
        parts[0]
    );
    assert_eq!(
        parts[2],
        Value::string("a"),
        "a freshly opened stream must read normally after the cancellation (runtime not wedged)"
    );
    assert_eq!(interp.runtime_resource_gate_count(), 0);
}
