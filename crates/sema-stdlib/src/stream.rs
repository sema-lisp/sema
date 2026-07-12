//! Stream primitives (`stream/*`).
//!
//! In-memory streams (`ByteBufferStream`/`StringStream`) are pure CPU/memory —
//! always synchronous, even inside `async/spawn`, no offload needed. File-backed
//! streams (`FileInputStream`/`FileOutputStream`, in `io_streams` below) wrap
//! real blocking I/O; `stream/read`, `stream/write`, `stream/read-line`,
//! `stream/copy`, `stream/flush`, and `stream/close` each check whether the
//! stream they were handed is file-backed AND `in_async_context()` before
//! deciding to offload — an in-memory stream falls straight through to the
//! unchanged synchronous path regardless of context.
//!
//! Unlike `sqlite.rs`/`proc.rs`, there is no separate keyed thread-local
//! registry: a stream is already reached through a unique `Rc<StreamBox>`
//! (the `Value` itself), so the CHECKOUT slot (`Available`/`CheckedOut`/
//! `Tombstone`, see `sqlite.rs`'s module doc comment for the pattern this
//! mirrors) lives directly on `FileInputStream`/`FileOutputStream` — the
//! `BufReader<File>`/`BufWriter<File>` is taken out of that slot for an
//! offload's duration and reinstalled by the poller, which calls
//! `notify_io_complete()` so a sibling queued on the SAME stream object can't
//! miss the wakeup. `stream/open-input`/`stream/open-output` offload the
//! initial `File::open`/`File::create` via `fs_offload` (`io.rs`) — there is
//! no existing stream to contend over, mirroring `db/open`.
//!
//! `stream/copy` between two FILE-backed streams deliberately does not
//! implement dual-checkout (it would need a canonical acquire order across
//! two independently-checked-out resources to avoid a would-be reverse copy
//! deadlocking against it) — that combination falls through to the existing
//! synchronous loop even inside async context: still correct, just a narrow,
//! documented blocking window for that one call. A copy with exactly one
//! file-backed side checks out only that side; the memory/stdio side is
//! read/written on the VM thread (fast, no I/O).
//!
//! At top level (no scheduler) every builtin keeps today's synchronous shape
//! byte-for-byte.

use std::any::Any;
use std::cell::{Cell, RefCell};

use sema_core::{check_arity, SemaError, SemaStream, Value};

use crate::register_fn;

#[cfg(not(target_arch = "wasm32"))]
use sema_core::{Caps, Env, Sandbox};

// ── In-memory stream implementations ─────────────────────────────

/// Read/write byte buffer. Writes append; reads consume from position.
#[derive(Debug)]
struct ByteBufferStream {
    buf: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}

impl ByteBufferStream {
    fn new(data: Vec<u8>) -> Self {
        ByteBufferStream {
            buf: RefCell::new(data),
            pos: Cell::new(0),
        }
    }

    fn empty() -> Self {
        Self::new(Vec::new())
    }
}

impl SemaStream for ByteBufferStream {
    fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError> {
        let data = self.buf.borrow();
        let pos = self.pos.get();
        let available = data.len().saturating_sub(pos);
        let n = buf.len().min(available);
        buf[..n].copy_from_slice(&data[pos..pos + n]);
        self.pos.set(pos + n);
        Ok(n)
    }

    fn write(&self, data: &[u8]) -> Result<usize, SemaError> {
        self.buf.borrow_mut().extend_from_slice(data);
        Ok(data.len())
    }

    fn available(&self) -> Result<bool, SemaError> {
        Ok(self.pos.get() < self.buf.borrow().len())
    }

    fn stream_type(&self) -> &'static str {
        "byte-buffer"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Read-only stream from a string's UTF-8 bytes.
#[derive(Debug)]
struct StringStream {
    data: Vec<u8>,
    pos: Cell<usize>,
}

impl SemaStream for StringStream {
    fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError> {
        let pos = self.pos.get();
        let available = self.data.len().saturating_sub(pos);
        let n = buf.len().min(available);
        buf[..n].copy_from_slice(&self.data[pos..pos + n]);
        self.pos.set(pos + n);
        Ok(n)
    }

    fn write(&self, _data: &[u8]) -> Result<usize, SemaError> {
        Err(SemaError::eval(
            "stream/write: stream is read-only (string stream)",
        ))
    }

    fn available(&self) -> Result<bool, SemaError> {
        Ok(self.pos.get() < self.data.len())
    }

    fn is_writable(&self) -> bool {
        false
    }

    fn stream_type(&self) -> &'static str {
        "string"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── Helper to extract stream from args ───────────────────────────

fn expect_stream(
    args: &[Value],
    fname: &str,
    idx: usize,
) -> Result<std::rc::Rc<sema_core::StreamBox>, SemaError> {
    args[idx]
        .as_stream_rc()
        .ok_or_else(|| SemaError::type_error("stream", args[idx].type_name()))
        .map_err(|e| e.with_hint(format!("{fname} expects a stream as argument {}", idx + 1)))
}

/// Downcast a borrowed stream inner to ByteBufferStream.
fn expect_byte_buffer<'a>(
    inner: &'a std::cell::Ref<'_, Box<dyn sema_core::SemaStream>>,
    stream_type: &str,
    fname: &str,
) -> Result<&'a ByteBufferStream, SemaError> {
    inner
        .as_any()
        .downcast_ref::<ByteBufferStream>()
        .ok_or_else(|| {
            SemaError::eval(format!(
                "{fname}: expected byte-buffer stream, got {stream_type} stream"
            ))
        })
}

// ── Registration ─────────────────────────────────────────────────

pub fn register(env: &sema_core::Env) {
    // --- predicate ---

    register_fn(env, "stream?", |args| {
        check_arity!(args, "stream?", 1);
        Ok(Value::bool(args[0].as_stream_rc().is_some()))
    });

    // --- core I/O ---

    register_fn(env, "stream/read", |args| {
        check_arity!(args, "stream/read", 2);
        let s = expect_stream(args, "stream/read", 0)?;
        let n = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        if n < 0 {
            return Err(SemaError::eval(format!(
                "stream/read: count must be non-negative, got {n}"
            )));
        }

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(v) = io_streams::maybe_async_read(&s, n as usize)? {
            return Ok(v);
        }

        let mut buf = vec![0u8; n as usize];
        let bytes_read = s.read(&mut buf)?;
        buf.truncate(bytes_read);
        Ok(Value::bytevector(buf))
    });

    register_fn(env, "stream/write", |args| {
        check_arity!(args, "stream/write", 2);
        let s = expect_stream(args, "stream/write", 0)?;
        let data = args[1]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(v) = io_streams::maybe_async_write(&s, data)? {
            return Ok(v);
        }

        let n = s.write(data)?;
        Ok(Value::int(n as i64))
    });

    register_fn(env, "stream/read-byte", |args| {
        check_arity!(args, "stream/read-byte", 1);
        let s = expect_stream(args, "stream/read-byte", 0)?;
        let mut buf = [0u8; 1];
        let n = s.read(&mut buf)?;
        if n == 0 {
            Ok(Value::nil())
        } else {
            Ok(Value::int(buf[0] as i64))
        }
    });

    register_fn(env, "stream/write-byte", |args| {
        check_arity!(args, "stream/write-byte", 2);
        let s = expect_stream(args, "stream/write-byte", 0)?;
        let b = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        if !(0..=255).contains(&b) {
            return Err(SemaError::eval(format!(
                "stream/write-byte: value {b} out of range 0..255"
            )));
        }
        s.write(&[b as u8])?;
        Ok(Value::nil())
    });

    register_fn(env, "stream/available?", |args| {
        check_arity!(args, "stream/available?", 1);
        let s = expect_stream(args, "stream/available?", 0)?;
        Ok(Value::bool(s.available()?))
    });

    register_fn(env, "stream/close", |args| {
        check_arity!(args, "stream/close", 1);
        let s = expect_stream(args, "stream/close", 0)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(v) = io_streams::maybe_async_close(&s)? {
            return Ok(v);
        }

        s.close()?;
        Ok(Value::nil())
    });

    register_fn(env, "stream/flush", |args| {
        check_arity!(args, "stream/flush", 1);
        let s = expect_stream(args, "stream/flush", 0)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(v) = io_streams::maybe_async_flush(&s)? {
            return Ok(v);
        }

        s.flush()?;
        Ok(Value::nil())
    });

    // --- introspection ---

    register_fn(env, "stream/readable?", |args| {
        check_arity!(args, "stream/readable?", 1);
        let s = expect_stream(args, "stream/readable?", 0)?;
        Ok(Value::bool(s.is_readable()))
    });

    register_fn(env, "stream/writable?", |args| {
        check_arity!(args, "stream/writable?", 1);
        let s = expect_stream(args, "stream/writable?", 0)?;
        Ok(Value::bool(s.is_writable()))
    });

    register_fn(env, "stream/type", |args| {
        check_arity!(args, "stream/type", 1);
        let s = expect_stream(args, "stream/type", 0)?;
        Ok(Value::string(s.stream_type()))
    });

    // --- constructors ---

    register_fn(env, "stream/byte-buffer", |args| {
        check_arity!(args, "stream/byte-buffer", 0);
        Ok(Value::stream(ByteBufferStream::empty()))
    });

    register_fn(env, "stream/from-string", |args| {
        check_arity!(args, "stream/from-string", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::stream(StringStream {
            data: s.as_bytes().to_vec(),
            pos: Cell::new(0),
        }))
    });

    register_fn(env, "stream/from-bytes", |args| {
        check_arity!(args, "stream/from-bytes", 1);
        let bv = args[0]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[0].type_name()))?;
        Ok(Value::stream(ByteBufferStream::new(bv.to_vec())))
    });

    // --- extraction ---

    register_fn(env, "stream/to-bytes", |args| {
        check_arity!(args, "stream/to-bytes", 1);
        let s = expect_stream(args, "stream/to-bytes", 0)?;
        let stype = s.stream_type(); // get before borrowing inner
        let inner = s.borrow_inner();
        let buf = expect_byte_buffer(&inner, stype, "stream/to-bytes")?;
        let bytes = buf.buf.borrow().clone();
        Ok(Value::bytevector(bytes))
    });

    register_fn(env, "stream/to-string", |args| {
        check_arity!(args, "stream/to-string", 1);
        let s = expect_stream(args, "stream/to-string", 0)?;
        let stype = s.stream_type(); // get before borrowing inner
        let inner = s.borrow_inner();
        let buf = expect_byte_buffer(&inner, stype, "stream/to-string")?;
        let bytes = buf.buf.borrow().clone();
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| SemaError::eval(format!("stream/to-string: invalid UTF-8: {e}")))?;
        Ok(Value::string(text))
    });

    // --- convenience (no I/O, always available) ---

    register_fn(env, "stream/read-all", |args| {
        check_arity!(args, "stream/read-all", 1);
        let s = expect_stream(args, "stream/read-all", 0)?;
        let mut result = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = s.read(&mut buf)?;
            if n == 0 {
                break;
            }
            result.extend_from_slice(&buf[..n]);
        }
        Ok(Value::bytevector(result))
    });

    register_fn(env, "stream/read-line", |args| {
        check_arity!(args, "stream/read-line", 1);
        let s = expect_stream(args, "stream/read-line", 0)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(v) = io_streams::maybe_async_read_line(&s)? {
            return Ok(v);
        }

        let mut line = Vec::new();
        let mut buf = [0u8; 1];
        loop {
            let n = s.read(&mut buf)?;
            if n == 0 {
                // EOF
                if line.is_empty() {
                    return Ok(Value::nil());
                }
                break;
            }
            if buf[0] == b'\n' {
                break;
            }
            line.push(buf[0]);
        }
        // Strip trailing \r if present
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let text = String::from_utf8(line)
            .map_err(|e| SemaError::eval(format!("stream/read-line: invalid UTF-8: {e}")))?;
        Ok(Value::string(&text))
    });

    register_fn(env, "stream/write-string", |args| {
        check_arity!(args, "stream/write-string", 2);
        let s = expect_stream(args, "stream/write-string", 0)?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let n = s.write(text.as_bytes())?;
        Ok(Value::int(n as i64))
    });

    register_fn(env, "stream/copy", |args| {
        check_arity!(args, "stream/copy", 2);
        let src = expect_stream(args, "stream/copy", 0)?;
        let dst = expect_stream(args, "stream/copy", 1)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(v) = io_streams::maybe_async_copy(&src, &dst)? {
            return Ok(v);
        }

        let mut total: usize = 0;
        let mut buf = [0u8; 8192];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            dst.write(&buf[..n])?;
            total += n;
        }
        Ok(Value::int(total as i64))
    });
}

// ── File and stdio streams (not available on wasm) ───────────────

#[cfg(not(target_arch = "wasm32"))]
mod io_streams {
    use super::*;
    use std::io::{BufRead, BufReader, BufWriter, Read, Write};
    use std::rc::Rc;

    use sema_core::{in_async_context, IoHandle, IoPoll, StreamBox};

    // `File`/`BufReader<File>`/`BufWriter<File>` move across the offload
    // boundary on every checkout. This compiles only if they stay `Send`; a
    // std change that broke it would fail here, not with an opaque
    // trait-bound error deep in `sema_io::io_spawn_blocking`.
    const _: fn() = || {
        fn assert_send<T: Send>() {}
        assert_send::<std::fs::File>();
        assert_send::<BufReader<std::fs::File>>();
        assert_send::<BufWriter<std::fs::File>>();
    };

    /// `op` was attempted while an offload had this stream's underlying file
    /// checked out.
    fn busy_err(op: &str) -> SemaError {
        SemaError::eval(format!(
            "{op}: stream is busy — another stream/* call is in flight on it"
        ))
        .with_hint("wait for the in-flight stream/* call on this stream before calling another")
    }

    /// `op` was attempted on a stream whose in-flight offload was cancelled.
    fn tombstone_err(op: &str, reason: &str) -> SemaError {
        SemaError::eval(format!("{op}: stream is no longer usable: {reason}"))
    }

    /// Pre-render `msg` through the same `SemaError::eval` constructor the
    /// sync path raises, so an async rejection's message text is
    /// substring-identical to what the sync path would display for the same
    /// failure (mirrors `eval_msg` in `sqlite.rs`).
    fn render(msg: String) -> String {
        SemaError::eval(msg).to_string()
    }

    /// Readable file stream with buffering.
    ///
    /// The checkout slot lives directly on the stream object rather than in
    /// a separate keyed registry (unlike `sqlite.rs`/`proc.rs`): the
    /// `Rc<StreamBox>` holding this stream already IS the unique handle.
    /// Closing an input stream needs no I/O (the default `SemaStream::close`
    /// is a no-op — the fd is released when the `Rc` finally drops), so
    /// `FileInSlot` tracks only busy/tombstoned, never "closed"; `stream/read`
    /// and `stream/read-line`'s async paths consult `StreamBox::is_closed()`
    /// directly before ever attempting a checkout.
    #[derive(Debug)]
    pub struct FileInputStream {
        slot: RefCell<FileInSlot>,
    }

    #[derive(Debug)]
    enum FileInSlot {
        Available(BufReader<std::fs::File>),
        CheckedOut,
        Tombstone(String),
    }

    impl FileInputStream {
        pub fn open(path: &str) -> Result<Self, SemaError> {
            let file = std::fs::File::open(path)
                .map_err(|e| SemaError::eval(format!("stream/open-input: {path}: {e}")))?;
            Ok(Self::from_reader(BufReader::new(file)))
        }

        fn from_reader(reader: BufReader<std::fs::File>) -> Self {
            FileInputStream {
                slot: RefCell::new(FileInSlot::Available(reader)),
            }
        }
    }

    impl SemaStream for FileInputStream {
        fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError> {
            match &mut *self.slot.borrow_mut() {
                FileInSlot::Available(r) => r
                    .read(buf)
                    .map_err(|e| SemaError::eval(format!("stream/read: I/O error: {e}"))),
                FileInSlot::CheckedOut => Err(busy_err("stream/read")),
                FileInSlot::Tombstone(msg) => Err(tombstone_err("stream/read", msg)),
            }
        }

        fn write(&self, _data: &[u8]) -> Result<usize, SemaError> {
            Err(SemaError::eval(
                "stream/write: stream is read-only (file-input stream)",
            ))
        }

        fn available(&self) -> Result<bool, SemaError> {
            // Check if the buffer has data; don't do a blocking read. A
            // checked-out or tombstoned slot has no reader to peek at —
            // report "nothing buffered" rather than erroring, matching the
            // trait's `available` being a best-effort, never-failing probe.
            match &*self.slot.borrow() {
                FileInSlot::Available(r) => Ok(!r.buffer().is_empty()),
                FileInSlot::CheckedOut | FileInSlot::Tombstone(_) => Ok(false),
            }
        }

        fn is_writable(&self) -> bool {
            false
        }

        fn stream_type(&self) -> &'static str {
            "file-input"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Writable file stream with buffering. See `FileInputStream`'s doc
    /// comment for the checkout-on-the-object design. Unlike the input side,
    /// closing IS real I/O (a final flush), so `FileOutSlot` tracks `Closed`
    /// as its own terminal (non-busy, non-tombstoned) state — reached only
    /// through `close()`, mirrored by the async path's `maybe_async_close`.
    #[derive(Debug)]
    pub struct FileOutputStream {
        slot: RefCell<FileOutSlot>,
    }

    #[derive(Debug)]
    enum FileOutSlot {
        Available(BufWriter<std::fs::File>),
        Closed,
        CheckedOut,
        Tombstone(String),
    }

    impl FileOutputStream {
        pub fn create(path: &str) -> Result<Self, SemaError> {
            let file = std::fs::File::create(path)
                .map_err(|e| SemaError::eval(format!("stream/open-output: {path}: {e}")))?;
            Ok(Self::from_writer(BufWriter::new(file)))
        }

        fn from_writer(writer: BufWriter<std::fs::File>) -> Self {
            FileOutputStream {
                slot: RefCell::new(FileOutSlot::Available(writer)),
            }
        }
    }

    impl SemaStream for FileOutputStream {
        fn read(&self, _buf: &mut [u8]) -> Result<usize, SemaError> {
            Err(SemaError::eval(
                "stream/read: stream is write-only (file-output stream)",
            ))
        }

        fn write(&self, data: &[u8]) -> Result<usize, SemaError> {
            match &mut *self.slot.borrow_mut() {
                FileOutSlot::Available(w) => w
                    .write(data)
                    .map_err(|e| SemaError::eval(format!("stream/write: I/O error: {e}"))),
                FileOutSlot::Closed => Err(SemaError::eval("stream/write: file stream is closed")),
                FileOutSlot::CheckedOut => Err(busy_err("stream/write")),
                FileOutSlot::Tombstone(msg) => Err(tombstone_err("stream/write", msg)),
            }
        }

        fn flush(&self) -> Result<(), SemaError> {
            match &mut *self.slot.borrow_mut() {
                FileOutSlot::Available(w) => w
                    .flush()
                    .map_err(|e| SemaError::eval(format!("stream/flush: I/O error: {e}"))),
                // Matches today: flushing an already-closed writer is a
                // silent no-op rather than an error (StreamBox's own closed
                // flag is what normally intercepts this first).
                FileOutSlot::Closed => Ok(()),
                FileOutSlot::CheckedOut => Err(busy_err("stream/flush")),
                FileOutSlot::Tombstone(msg) => Err(tombstone_err("stream/flush", msg)),
            }
        }

        fn close(&self) -> Result<(), SemaError> {
            let mut slot = self.slot.borrow_mut();
            match &mut *slot {
                FileOutSlot::Available(w) => {
                    // On a flush error the slot is left untouched (still
                    // `Available`) rather than transitioning to `Closed` —
                    // matches today's behavior exactly (the old `self.flush()?`
                    // early-return left `writer` as `Some`, i.e. still open).
                    w.flush()
                        .map_err(|e| SemaError::eval(format!("stream/close: I/O error: {e}")))?;
                    *slot = FileOutSlot::Closed;
                    Ok(())
                }
                FileOutSlot::Closed => Ok(()), // double-close is a no-op
                FileOutSlot::CheckedOut => Err(busy_err("stream/close")),
                FileOutSlot::Tombstone(msg) => Err(tombstone_err("stream/close", msg)),
            }
        }

        fn is_readable(&self) -> bool {
            false
        }

        fn stream_type(&self) -> &'static str {
            "file-output"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    // ── Async offload machinery ───────────────────────────────────

    /// What crosses the thread boundary from an offloaded file op back to
    /// the poller: the reinstalled resource (`BufReader<File>` or
    /// `BufWriter<File>`) plus the op's owned `Send` result. Mirrors
    /// `sqlite.rs`'s `ConnOpOutcome`.
    struct FileOpOutcome<R, T> {
        resource: R,
        result: Result<T, String>,
    }

    /// The two phases a file-checkout offload's `IoHandle` cycles through —
    /// identical shape to `sqlite.rs`'s `ConnPhase`/`proc.rs`'s `WaitPhase`.
    enum FilePhase<R, T> {
        /// Waiting for the slot to become `Available`. Re-checked every
        /// poll; never mutates anything beyond that check, so aborting here
        /// is a true no-op — nothing was ever taken out.
        Acquire,
        /// Holding the checkout; `op` is running on the I/O pool. Resolves
        /// with the reinstalled resource plus the op's result.
        Running(tokio::sync::oneshot::Receiver<FileOpOutcome<R, T>>),
    }

    /// Offload one blocking op on a file-backed stream through the CHECKOUT
    /// pattern (see the module doc comment). `try_take` is the Acquire-phase
    /// probe: `Ok(None)` = still busy (keep polling), `Ok(Some(resource))` =
    /// acquired, `Err` = terminal (tombstoned/closed). `op` runs on the I/O
    /// pool against the acquired resource; `reinstall` puts it back
    /// `Available` once `op` returns (success or failure — an op error never
    /// tombstones the stream, only a cancelled-in-flight offload or a
    /// vanished worker does). `decode` turns the owned `Send` result into
    /// the final `Value` on the VM thread — fallible, since e.g.
    /// `stream/copy`'s decode step writes into the OTHER (memory-side)
    /// stream, which can itself reject the write. Returns `Ok(nil)` after
    /// arming the yield signal; the scheduler delivers the real value on
    /// resume.
    fn checkout_offload<R: Send + 'static, T: Send + 'static>(
        op_name: &'static str,
        try_take: impl Fn() -> Result<Option<R>, String> + 'static,
        reinstall: impl Fn(R) + 'static,
        tombstone: impl Fn(String) + Clone + 'static,
        op: impl FnOnce(&mut R) -> Result<T, String> + Send + 'static,
        decode: impl Fn(T) -> Result<Value, SemaError> + 'static,
    ) -> Result<Value, SemaError> {
        use tokio::sync::oneshot::error::TryRecvError;

        // Vestigial under CALL_NATIVE (the scheduler delivers the resume
        // value via `replace_stack_top`, not by re-invoking this native),
        // but kept for symmetry with the shipped `async/await` yield pattern.
        if let Some(v) = sema_core::take_resume_value() {
            return Ok(v);
        }

        let phase = Rc::new(RefCell::new(FilePhase::<R, T>::Acquire));
        let phase_for_poll = phase.clone();
        let mut op_holder = Some(op);
        let tombstone_poll = tombstone.clone();

        let poll = move || -> IoPoll {
            loop {
                let is_acquire = matches!(&*phase_for_poll.borrow(), FilePhase::Acquire);
                if is_acquire {
                    match try_take() {
                        Ok(None) => return IoPoll::Pending,
                        Err(msg) => return IoPoll::Ready(Err(msg)),
                        Ok(Some(resource)) => {
                            let op = op_holder
                                .take()
                                .expect("checkout_offload's op is consumed exactly once");
                            let (tx, rx) = tokio::sync::oneshot::channel();
                            let mut resource = resource;
                            sema_io::io_spawn_blocking(move || {
                                let result = op(&mut resource);
                                let _ = tx.send(FileOpOutcome { resource, result });
                                // Wake the parked VM thread so it re-polls promptly.
                                sema_core::notify_io_complete();
                            });
                            *phase_for_poll.borrow_mut() = FilePhase::Running(rx);
                            // Fall through: poll the freshly spawned receiver
                            // immediately instead of wasting a scheduler tick.
                        }
                    }
                } else {
                    let mut phase_ref = phase_for_poll.borrow_mut();
                    let FilePhase::Running(rx) = &mut *phase_ref else {
                        unreachable!("Acquire handled above")
                    };
                    return match rx.try_recv() {
                        Err(TryRecvError::Empty) => IoPoll::Pending,
                        Ok(outcome) => {
                            drop(phase_ref);
                            reinstall(outcome.resource);
                            // MANDATORY lost-wakeup guard: a sibling queued
                            // on this same stream (still in `Acquire`) may
                            // have polled Pending earlier in this scheduler
                            // sweep — without this it would park until an
                            // unrelated wakeup.
                            sema_core::notify_io_complete();
                            match outcome.result {
                                Ok(t) => match decode(t) {
                                    Ok(v) => IoPoll::Ready(Ok(v)),
                                    Err(e) => IoPoll::Ready(Err(e.to_string())),
                                },
                                Err(msg) => IoPoll::Ready(Err(msg)),
                            }
                        }
                        Err(TryRecvError::Closed) => {
                            drop(phase_ref);
                            tombstone_poll("the I/O worker terminated unexpectedly".to_string());
                            IoPoll::Ready(Err(format!("{op_name}: I/O worker dropped")))
                        }
                    };
                }
            }
        };

        let phase_for_abort = phase.clone();
        let io_handle = Rc::new(IoHandle::with_abort(poll, move || {
            // Acquire-phase abort: no-op — nothing was ever checked out.
            // Running-phase abort: best-effort, matching every other
            // spawn_blocking-based offload in this codebase (see
            // `IoHandle::with_abort`'s doc comment) — the blocking op keeps
            // running unattended on the worker; the stream is tombstoned so
            // a later access errors clearly instead of the slot staying
            // `CheckedOut` forever with no one left to reinstall it.
            if matches!(*phase_for_abort.borrow(), FilePhase::Running(_)) {
                tombstone(format!(
                    "{op_name} was cancelled while in flight; the stream cannot be reclaimed \
                     — stream/close frees the handle"
                ));
            }
        }));
        sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(io_handle));
        Ok(Value::nil())
    }

    fn try_checkout_input(
        stream: &Rc<StreamBox>,
        op: &str,
    ) -> Result<Option<BufReader<std::fs::File>>, String> {
        let inner = stream.borrow_inner();
        let fis = inner
            .as_any()
            .downcast_ref::<FileInputStream>()
            .expect("caller already verified stream_type() == \"file-input\"");
        let mut slot = fis.slot.borrow_mut();
        match &mut *slot {
            FileInSlot::Available(_) => {
                let FileInSlot::Available(r) =
                    std::mem::replace(&mut *slot, FileInSlot::CheckedOut)
                else {
                    unreachable!("just matched Available")
                };
                Ok(Some(r))
            }
            FileInSlot::CheckedOut => Ok(None),
            FileInSlot::Tombstone(msg) => Err(tombstone_err(op, msg).to_string()),
        }
    }

    fn reinstall_input(stream: &Rc<StreamBox>, reader: BufReader<std::fs::File>) {
        let inner = stream.borrow_inner();
        if let Some(fis) = inner.as_any().downcast_ref::<FileInputStream>() {
            *fis.slot.borrow_mut() = FileInSlot::Available(reader);
        }
    }

    fn tombstone_input(stream: &Rc<StreamBox>, msg: String) {
        let inner = stream.borrow_inner();
        if let Some(fis) = inner.as_any().downcast_ref::<FileInputStream>() {
            *fis.slot.borrow_mut() = FileInSlot::Tombstone(msg);
        }
    }

    fn try_checkout_output(
        stream: &Rc<StreamBox>,
        op: &str,
    ) -> Result<Option<BufWriter<std::fs::File>>, String> {
        let inner = stream.borrow_inner();
        let fos = inner
            .as_any()
            .downcast_ref::<FileOutputStream>()
            .expect("caller already verified stream_type() == \"file-output\"");
        let mut slot = fos.slot.borrow_mut();
        match &mut *slot {
            FileOutSlot::Available(_) => {
                let FileOutSlot::Available(w) =
                    std::mem::replace(&mut *slot, FileOutSlot::CheckedOut)
                else {
                    unreachable!("just matched Available")
                };
                Ok(Some(w))
            }
            FileOutSlot::CheckedOut => Ok(None),
            // Reachable when a copy targets an already-closed file stream
            // (the dst side is never checked with `StreamBox::is_closed()`
            // up front — see `maybe_async_copy`), or the narrow concurrent-
            // close race documented on `FileOutSlot`. Phrased exactly like
            // `StreamBox::write`'s own closed-stream message, since every
            // caller of this function is either writing or finalizing a
            // write-adjacent op (`stream/write`/`stream/flush`/`stream/copy`
            // /`stream/close`).
            FileOutSlot::Closed => Err(render(format!("{op}: stream is closed"))),
            FileOutSlot::Tombstone(msg) => Err(tombstone_err(op, msg).to_string()),
        }
    }

    fn reinstall_output(stream: &Rc<StreamBox>, writer: BufWriter<std::fs::File>) {
        let inner = stream.borrow_inner();
        if let Some(fos) = inner.as_any().downcast_ref::<FileOutputStream>() {
            *fos.slot.borrow_mut() = FileOutSlot::Available(writer);
        }
    }

    fn tombstone_output(stream: &Rc<StreamBox>, msg: String) {
        let inner = stream.borrow_inner();
        if let Some(fos) = inner.as_any().downcast_ref::<FileOutputStream>() {
            *fos.slot.borrow_mut() = FileOutSlot::Tombstone(msg);
        }
    }

    // ── Dispatch: called from `register()`'s builtin closures ──────
    //
    // Each returns `Ok(None)` when the caller should fall through to the
    // unchanged synchronous path: not in async context, or the stream isn't
    // file-backed (in-memory streams stay sync-fast even inside async
    // context — there's nothing to offload).

    pub(super) fn maybe_async_read(
        stream: &Rc<StreamBox>,
        n: usize,
    ) -> Result<Option<Value>, SemaError> {
        if !in_async_context() || stream.stream_type() != "file-input" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/read: stream is closed"));
        }
        let s1 = stream.clone();
        let s2 = stream.clone();
        let s3 = stream.clone();
        let v = checkout_offload(
            "stream/read",
            move || try_checkout_input(&s1, "stream/read"),
            move |r| reinstall_input(&s2, r),
            move |msg| tombstone_input(&s3, msg),
            move |reader: &mut BufReader<std::fs::File>| -> Result<Vec<u8>, String> {
                let mut buf = vec![0u8; n];
                let read = reader
                    .read(&mut buf)
                    .map_err(|e| render(format!("stream/read: I/O error: {e}")))?;
                buf.truncate(read);
                Ok(buf)
            },
            |bytes: Vec<u8>| -> Result<Value, SemaError> { Ok(Value::bytevector(bytes)) },
        )?;
        Ok(Some(v))
    }

    pub(super) fn maybe_async_read_line(
        stream: &Rc<StreamBox>,
    ) -> Result<Option<Value>, SemaError> {
        if !in_async_context() || stream.stream_type() != "file-input" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/read-line: stream is closed"));
        }
        let s1 = stream.clone();
        let s2 = stream.clone();
        let s3 = stream.clone();
        let v = checkout_offload(
            "stream/read-line",
            move || try_checkout_input(&s1, "stream/read-line"),
            move |r| reinstall_input(&s2, r),
            move |msg| tombstone_input(&s3, msg),
            move |reader: &mut BufReader<std::fs::File>| -> Result<Option<String>, String> {
                let mut line = Vec::new();
                let n = reader
                    .read_until(b'\n', &mut line)
                    .map_err(|e| render(format!("stream/read-line: I/O error: {e}")))?;
                if n == 0 {
                    return Ok(None); // EOF, nothing read at all
                }
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                String::from_utf8(line)
                    .map(Some)
                    .map_err(|e| render(format!("stream/read-line: invalid UTF-8: {e}")))
            },
            |line: Option<String>| -> Result<Value, SemaError> {
                Ok(match line {
                    None => Value::nil(),
                    Some(s) => Value::string(&s),
                })
            },
        )?;
        Ok(Some(v))
    }

    pub(super) fn maybe_async_write(
        stream: &Rc<StreamBox>,
        data: &[u8],
    ) -> Result<Option<Value>, SemaError> {
        if !in_async_context() || stream.stream_type() != "file-output" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/write: stream is closed"));
        }
        let data = data.to_vec();
        let s1 = stream.clone();
        let s2 = stream.clone();
        let s3 = stream.clone();
        let v = checkout_offload(
            "stream/write",
            move || try_checkout_output(&s1, "stream/write"),
            move |w| reinstall_output(&s2, w),
            move |msg| tombstone_output(&s3, msg),
            move |writer: &mut BufWriter<std::fs::File>| -> Result<usize, String> {
                writer
                    .write(&data)
                    .map_err(|e| render(format!("stream/write: I/O error: {e}")))
            },
            |n: usize| -> Result<Value, SemaError> { Ok(Value::int(n as i64)) },
        )?;
        Ok(Some(v))
    }

    pub(super) fn maybe_async_flush(stream: &Rc<StreamBox>) -> Result<Option<Value>, SemaError> {
        if !in_async_context() || stream.stream_type() != "file-output" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/flush: stream is closed"));
        }
        let s1 = stream.clone();
        let s2 = stream.clone();
        let s3 = stream.clone();
        let v = checkout_offload(
            "stream/flush",
            move || try_checkout_output(&s1, "stream/flush"),
            move |w| reinstall_output(&s2, w),
            move |msg| tombstone_output(&s3, msg),
            move |writer: &mut BufWriter<std::fs::File>| -> Result<(), String> {
                writer
                    .flush()
                    .map_err(|e| render(format!("stream/flush: I/O error: {e}")))
            },
            |()| -> Result<Value, SemaError> { Ok(Value::nil()) },
        )?;
        Ok(Some(v))
    }

    pub(super) fn maybe_async_close(stream: &Rc<StreamBox>) -> Result<Option<Value>, SemaError> {
        if !in_async_context() || stream.stream_type() != "file-output" {
            return Ok(None);
        }
        if stream.is_closed() {
            // Matches `StreamBox::close`'s own idempotency.
            return Ok(Some(Value::nil()));
        }
        let s1 = stream.clone();
        let s2 = stream.clone();
        let s3 = stream.clone();
        let stream_for_finish = stream.clone();
        let v = checkout_offload(
            "stream/close",
            move || try_checkout_output(&s1, "stream/close"),
            move |w| reinstall_output(&s2, w),
            move |msg| tombstone_output(&s3, msg),
            move |writer: &mut BufWriter<std::fs::File>| -> Result<(), String> {
                writer
                    .flush()
                    .map_err(|e| render(format!("stream/close: I/O error: {e}")))
            },
            move |()| -> Result<Value, SemaError> {
                // The buffer is already flushed (above, off the VM thread);
                // this final call is now cheap (a fast no-op re-flush) and
                // transitions the slot to `Closed` while also flipping
                // `StreamBox`'s own closed flag — the single source of
                // truth `stream/read`/`write`/`flush` consult.
                stream_for_finish.close()?;
                Ok(Value::nil())
            },
        )?;
        Ok(Some(v))
    }

    /// `stream/copy`'s async dispatch. See the module doc comment for the
    /// three-way policy (both memory: sync; one file: checkout that side
    /// only; both file: sync fallback, documented and deliberate).
    pub(super) fn maybe_async_copy(
        src: &Rc<StreamBox>,
        dst: &Rc<StreamBox>,
    ) -> Result<Option<Value>, SemaError> {
        if !in_async_context() {
            return Ok(None);
        }
        let src_file = src.stream_type() == "file-input";
        let dst_file = dst.stream_type() == "file-output";
        if !src_file && !dst_file {
            return Ok(None);
        }
        if src_file && dst_file {
            return Ok(None);
        }

        if src_file {
            if src.is_closed() {
                return Err(SemaError::eval("stream/read: stream is closed"));
            }
            let s1 = src.clone();
            let s2 = src.clone();
            let s3 = src.clone();
            let dst_for_decode = dst.clone();
            let v = checkout_offload(
                "stream/copy",
                move || try_checkout_input(&s1, "stream/copy"),
                move |r| reinstall_input(&s2, r),
                move |msg| tombstone_input(&s3, msg),
                move |reader: &mut BufReader<std::fs::File>| -> Result<Vec<u8>, String> {
                    let mut out = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        let n = reader
                            .read(&mut chunk)
                            .map_err(|e| render(format!("stream/copy: I/O error: {e}")))?;
                        if n == 0 {
                            break;
                        }
                        out.extend_from_slice(&chunk[..n]);
                    }
                    Ok(out)
                },
                move |bytes: Vec<u8>| -> Result<Value, SemaError> {
                    let total = bytes.len();
                    // Mirrors the sync loop, which only ever calls
                    // `dst.write` when a read actually returned bytes —
                    // an already-EOF/empty src never touches dst at all.
                    if !bytes.is_empty() {
                        dst_for_decode.write(&bytes)?;
                    }
                    Ok(Value::int(total as i64))
                },
            )?;
            return Ok(Some(v));
        }

        // dst_file: src is memory/stdio. Read everything from src NOW (fast,
        // sync, on the VM thread — never real I/O), then offload the write.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = src.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        if buf.is_empty() {
            // Nothing read — the sync loop would never have touched dst
            // either, so there's nothing to offload.
            return Ok(Some(Value::int(0)));
        }
        let total = buf.len();
        let s1 = dst.clone();
        let s2 = dst.clone();
        let s3 = dst.clone();
        let v = checkout_offload(
            "stream/copy",
            move || try_checkout_output(&s1, "stream/write"),
            move |w| reinstall_output(&s2, w),
            move |msg| tombstone_output(&s3, msg),
            move |writer: &mut BufWriter<std::fs::File>| -> Result<(), String> {
                writer
                    .write_all(&buf)
                    .map_err(|e| render(format!("stream/copy: I/O error: {e}")))
            },
            move |()| -> Result<Value, SemaError> { Ok(Value::int(total as i64)) },
        )?;
        Ok(Some(v))
    }

    /// `stream/open-input`'s dispatch: async context offloads the blocking
    /// `File::open` via `fs_offload` (`io.rs`) — mirrors `db/open`, there is
    /// no existing stream to contend over. Sync stays today's shape.
    pub(super) fn open_input(path: &str) -> Result<Value, SemaError> {
        if in_async_context() {
            let path = path.to_string();
            return crate::io::fs_offload(
                move || {
                    std::fs::File::open(&path)
                        .map(BufReader::new)
                        .map_err(|e| render(format!("stream/open-input: {path}: {e}")))
                },
                |reader| Value::stream(FileInputStream::from_reader(reader)),
            );
        }
        Ok(Value::stream(FileInputStream::open(path)?))
    }

    /// `stream/open-output`'s dispatch — see `open_input`.
    pub(super) fn open_output(path: &str) -> Result<Value, SemaError> {
        if in_async_context() {
            let path = path.to_string();
            return crate::io::fs_offload(
                move || {
                    std::fs::File::create(&path)
                        .map(BufWriter::new)
                        .map_err(|e| render(format!("stream/open-output: {path}: {e}")))
                },
                |writer| Value::stream(FileOutputStream::from_writer(writer)),
            );
        }
        Ok(Value::stream(FileOutputStream::create(path)?))
    }

    /// Stdin stream — readable, close is a no-op.
    #[derive(Debug)]
    pub struct StdinStream;

    impl SemaStream for StdinStream {
        fn read(&self, buf: &mut [u8]) -> Result<usize, SemaError> {
            std::io::stdin()
                .read(buf)
                .map_err(|e| SemaError::eval(format!("stream/read: stdin: {e}")))
        }

        fn write(&self, _data: &[u8]) -> Result<usize, SemaError> {
            Err(SemaError::eval("stream/write: *stdin* is read-only"))
        }

        fn is_writable(&self) -> bool {
            false
        }

        fn stream_type(&self) -> &'static str {
            "stdin"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Stdout stream — writable, close is a no-op.
    #[derive(Debug)]
    pub struct StdoutStream;

    impl SemaStream for StdoutStream {
        fn read(&self, _buf: &mut [u8]) -> Result<usize, SemaError> {
            Err(SemaError::eval("stream/read: *stdout* is write-only"))
        }

        fn write(&self, data: &[u8]) -> Result<usize, SemaError> {
            let text = String::from_utf8_lossy(data);
            sema_core::write_stdout(&text);
            Ok(data.len())
        }

        fn flush(&self) -> Result<(), SemaError> {
            let _ = std::io::stdout().flush();
            Ok(())
        }

        fn is_readable(&self) -> bool {
            false
        }

        fn stream_type(&self) -> &'static str {
            "stdout"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Stderr stream — writable, close is a no-op.
    #[derive(Debug)]
    pub struct StderrStream;

    impl SemaStream for StderrStream {
        fn read(&self, _buf: &mut [u8]) -> Result<usize, SemaError> {
            Err(SemaError::eval("stream/read: *stderr* is write-only"))
        }

        fn write(&self, data: &[u8]) -> Result<usize, SemaError> {
            let text = String::from_utf8_lossy(data);
            sema_core::write_stderr(&text);
            Ok(data.len())
        }

        fn flush(&self) -> Result<(), SemaError> {
            let _ = std::io::stderr().flush();
            Ok(())
        }

        fn is_readable(&self) -> bool {
            false
        }

        fn stream_type(&self) -> &'static str {
            "stderr"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn register_io(env: &Env, sandbox: &Sandbox) {
    use io_streams::*;

    // --- file stream constructors (sandbox-gated) ---

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_READ,
        "stream/open-input",
        &[0],
        |args| {
            check_arity!(args, "stream/open-input", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            io_streams::open_input(path)
        },
    );

    crate::register_fn_path_gated(
        env,
        sandbox,
        Caps::FS_WRITE,
        "stream/open-output",
        &[0],
        |args| {
            check_arity!(args, "stream/open-output", 1);
            let path = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            io_streams::open_output(path)
        },
    );

    // --- global stdio streams ---

    env.set(sema_core::intern("*stdin*"), Value::stream(StdinStream));
    env.set(sema_core::intern("*stdout*"), Value::stream(StdoutStream));
    env.set(sema_core::intern("*stderr*"), Value::stream(StderrStream));
}
