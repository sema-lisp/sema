//! Stream primitives (`stream/*`).
//!
//! In-memory streams (`ByteBufferStream`/`StringStream`) are pure CPU/memory —
//! always synchronous, even inside `async/spawn`, no offload needed. File-backed
//! streams (`FileInputStream`/`FileOutputStream`, in `io_streams` below) wrap
//! real blocking I/O; `stream/read`, `stream/write`, `stream/read-line`,
//! `stream/copy`, `stream/flush`, and `stream/close` each check whether the
//! stream they were handed is file-backed AND `in_runtime_quantum()` before
//! deciding to offload — an in-memory stream falls straight through to the
//! unchanged synchronous path regardless of context.
//!
//! Unlike `sqlite.rs`/`proc.rs`, there is no separate keyed thread-local
//! registry: a stream is already reached through a unique `Rc<StreamBox>`
//! (the `Value` itself), so the CHECKOUT slot (`Available`/`CheckedOut`/
//! `Tombstone`) lives directly on `FileInputStream`/`FileOutputStream`, guarded
//! by a per-object `ResourceGate` (its id cached on the stream) that serializes
//! concurrent ops FIFO. The offload runs through
//! `crate::runtime_offload::checkout_external` (see `sqlite.rs`'s module doc
//! comment for the pattern this mirrors): acquire the gate, take the
//! `BufReader<File>`/`BufWriter<File>` out of the slot, run the blocking op on
//! the executor's blocking tier, then reinstall it and release the gate. A
//! sibling op on the SAME stream object parks FIFO on the gate; a mid-flight
//! cancel tombstones the slot (best-effort). `stream/open-input`/
//! `stream/open-output` offload the initial `File::open`/`File::create` on a
//! quarantined-bounded External wait (`crate::io::quarantined_compute`, `io.rs`)
//! — there is no existing stream to contend over, mirroring `db/open`. Finite
//! stdin reads use that path; whole-stdin aggregation uses nonblocking readiness
//! checks plus structural timer wakes, so cancellation never leaves a worker
//! pinned on an open pipe.
//!
//! `stream/read-all` and `stream/copy` accept an optional final `max-bytes`
//! argument and default to 256 MiB. The captured cap is checked before every
//! aggregate-buffer growth or destination write. A copy with exactly one
//! file-backed side checks out only that side. File-to-file copy inside a
//! runtime quantum fails promptly with bounded-chunk guidance: safely supporting
//! it requires canonical dual-gate acquisition, and it must never fall through
//! to a VM-thread EOF loop. The host value ABI retains the bounded synchronous
//! loop for compatibility.
//!
//! Direct host calls through the value ABI keep a bounded synchronous path.

use std::any::Any;
use std::cell::{Cell, RefCell};

use sema_core::runtime::NativeOutcome;
use sema_core::{check_arity, SemaError, SemaStream, Value};

use crate::register_fn;

/// Default maximum for one `stream/read-all` or `stream/copy` call. Callers can
/// pass a smaller or larger explicit maximum as the final argument, but no
/// aggregation path is ever unbounded.
const STREAM_AGGREGATION_BYTE_CAP_DEFAULT: usize = 256 * 1024 * 1024;
const STREAM_CHUNK_BYTES: usize = 8192;

fn aggregation_cap(args: &[Value], index: usize, op: &str) -> Result<usize, SemaError> {
    let Some(value) = args.get(index) else {
        return Ok(STREAM_AGGREGATION_BYTE_CAP_DEFAULT);
    };
    let cap = value
        .as_int()
        .ok_or_else(|| SemaError::type_error("non-negative integer", value.type_name()))?;
    usize::try_from(cap).map_err(|_| {
        SemaError::eval(format!(
            "{op}: max-bytes must be a non-negative integer representable on this platform, got {cap}"
        ))
    })
}

fn aggregation_cap_message(op: &str, cap: usize) -> String {
    format!("{op}: input exceeds the configured {cap}-byte cap")
}

fn aggregation_cap_error(op: &str, cap: usize) -> SemaError {
    SemaError::eval(aggregation_cap_message(op, cap)).with_hint(
        "process the stream with stream/read and stream/write in bounded chunks, or raise max-bytes",
    )
}

/// Extend an aggregation buffer only after proving the incoming chunk fits.
/// The check precedes `try_reserve_exact`, so an over-cap chunk cannot trigger a
/// capacity growth even when it is only one byte beyond the boundary.
fn extend_aggregation(
    output: &mut Vec<u8>,
    chunk: &[u8],
    cap: usize,
    op: &str,
) -> Result<(), SemaError> {
    if chunk.len() > cap.saturating_sub(output.len()) {
        return Err(aggregation_cap_error(op, cap));
    }
    let required = output.len() + chunk.len();
    if required > output.capacity() {
        let geometric = output
            .capacity()
            .max(STREAM_CHUNK_BYTES)
            .saturating_mul(2)
            .min(cap);
        let target = required.max(geometric);
        let additional = target - output.len();
        output.try_reserve_exact(additional).map_err(|error| {
            SemaError::eval(format!(
                "{op}: could not reserve {additional} bytes within the {cap}-byte cap: {error}"
            ))
        })?;
    }
    output.extend_from_slice(chunk);
    Ok(())
}

fn checked_copy_total(total: usize, next: usize, cap: usize) -> Result<usize, SemaError> {
    if next > cap.saturating_sub(total) {
        Err(aggregation_cap_error("stream/copy", cap))
    } else {
        Ok(total + next)
    }
}

/// Request only the still-allowed bytes plus one overflow witness. Reading the
/// witness is enough to reject an over-cap source without consuming a whole
/// excess chunk.
fn capped_read_len(current: usize, cap: usize) -> usize {
    STREAM_CHUNK_BYTES.min(cap.saturating_sub(current).saturating_add(1).max(1))
}

/// Register a builtin whose body speaks the runtime native ABI
/// (`NativeResult`), so its async branch can return a `NativeOutcome::Suspend`
/// (a gate-guarded checkout offload) directly. Mirrors
/// `crate::register_runtime_fn_path_gated` minus the sandbox/path gating (the
/// `stream/*` builtins are ungated). The body is exposed under BOTH ABIs: the
/// runtime callback returns the body's `NativeOutcome` structurally, and the
/// value callback accepts the plain `Return` produced for bare/top-level eval.
fn register_runtime_fn(
    env: &sema_core::Env,
    name: &'static str,
    f: impl Fn(&[Value]) -> sema_core::runtime::NativeResult + 'static,
) {
    use sema_core::runtime::NativeOutcome;
    let body = std::rc::Rc::new(f);
    let for_func = body.clone();
    let for_runtime = body;
    env.set(
        sema_core::intern(name),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            name,
            move |args| match for_func(args)? {
                NativeOutcome::Return(value) => Ok(value),
                _ => Err(sema_core::SemaError::eval(format!(
                    "{name}: native suspended outside the cooperative runtime"
                ))),
            },
            move |_ctx, args| for_runtime(args),
        )),
    );
}

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

    register_runtime_fn(env, "stream/read", |args| {
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
        if let Some(outcome) = io_streams::maybe_async_read(&s, n as usize)? {
            return Ok(outcome);
        }

        let mut buf = vec![0u8; n as usize];
        let bytes_read = s.read(&mut buf)?;
        buf.truncate(bytes_read);
        Ok(NativeOutcome::Return(Value::bytevector(buf)))
    });

    register_runtime_fn(env, "stream/write", |args| {
        check_arity!(args, "stream/write", 2);
        let s = expect_stream(args, "stream/write", 0)?;
        let data = args[1]
            .as_bytevector()
            .ok_or_else(|| SemaError::type_error("bytevector", args[1].type_name()))?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_write(&s, data)? {
            return Ok(outcome);
        }

        let n = s.write(data)?;
        Ok(NativeOutcome::Return(Value::int(n as i64)))
    });

    register_runtime_fn(env, "stream/read-byte", |args| {
        check_arity!(args, "stream/read-byte", 1);
        let s = expect_stream(args, "stream/read-byte", 0)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_read_byte(&s)? {
            return Ok(outcome);
        }

        let mut buf = [0u8; 1];
        let n = s.read(&mut buf)?;
        if n == 0 {
            Ok(NativeOutcome::Return(Value::nil()))
        } else {
            Ok(NativeOutcome::Return(Value::int(buf[0] as i64)))
        }
    });

    register_runtime_fn(env, "stream/write-byte", |args| {
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

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_write_byte(&s, b as u8)? {
            return Ok(outcome);
        }

        s.write(&[b as u8])?;
        Ok(NativeOutcome::Return(Value::nil()))
    });

    register_fn(env, "stream/available?", |args| {
        check_arity!(args, "stream/available?", 1);
        let s = expect_stream(args, "stream/available?", 0)?;
        Ok(Value::bool(s.available()?))
    });

    register_runtime_fn(env, "stream/close", |args| {
        check_arity!(args, "stream/close", 1);
        let s = expect_stream(args, "stream/close", 0)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_close(&s)? {
            return Ok(outcome);
        }

        s.close()?;
        Ok(NativeOutcome::Return(Value::nil()))
    });

    register_runtime_fn(env, "stream/flush", |args| {
        check_arity!(args, "stream/flush", 1);
        let s = expect_stream(args, "stream/flush", 0)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_flush(&s)? {
            return Ok(outcome);
        }

        s.flush()?;
        Ok(NativeOutcome::Return(Value::nil()))
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

    register_runtime_fn(env, "stream/read-all", |args| {
        check_arity!(args, "stream/read-all", 1..=2);
        let s = expect_stream(args, "stream/read-all", 0)?;
        let cap = aggregation_cap(args, 1, "stream/read-all")?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_read_all(&s, cap)? {
            return Ok(outcome);
        }

        let mut result = Vec::new();
        let mut buf = [0u8; STREAM_CHUNK_BYTES];
        loop {
            let read_len = capped_read_len(result.len(), cap);
            let n = s.read(&mut buf[..read_len])?;
            if n == 0 {
                break;
            }
            extend_aggregation(&mut result, &buf[..n], cap, "stream/read-all")?;
        }
        Ok(NativeOutcome::Return(Value::bytevector(result)))
    });

    register_runtime_fn(env, "stream/read-line", |args| {
        check_arity!(args, "stream/read-line", 1);
        let s = expect_stream(args, "stream/read-line", 0)?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_read_line(&s)? {
            return Ok(outcome);
        }

        let mut line = Vec::new();
        let mut buf = [0u8; 1];
        loop {
            let n = s.read(&mut buf)?;
            if n == 0 {
                // EOF
                if line.is_empty() {
                    return Ok(NativeOutcome::Return(Value::nil()));
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
        Ok(NativeOutcome::Return(Value::string(&text)))
    });

    register_runtime_fn(env, "stream/write-string", |args| {
        check_arity!(args, "stream/write-string", 2);
        let s = expect_stream(args, "stream/write-string", 0)?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_write(&s, text.as_bytes())? {
            return Ok(outcome);
        }

        let n = s.write(text.as_bytes())?;
        Ok(NativeOutcome::Return(Value::int(n as i64)))
    });

    register_runtime_fn(env, "stream/copy", |args| {
        check_arity!(args, "stream/copy", 2..=3);
        let src = expect_stream(args, "stream/copy", 0)?;
        let dst = expect_stream(args, "stream/copy", 1)?;
        let cap = aggregation_cap(args, 2, "stream/copy")?;

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(outcome) = io_streams::maybe_async_copy(&src, &dst, cap)? {
            return Ok(outcome);
        }

        let mut total: usize = 0;
        let mut buf = [0u8; STREAM_CHUNK_BYTES];
        loop {
            let read_len = capped_read_len(total, cap);
            let n = src.read(&mut buf[..read_len])?;
            if n == 0 {
                break;
            }
            let next_total = checked_copy_total(total, n, cap)?;
            dst.write(&buf[..n])?;
            total = next_total;
        }
        Ok(NativeOutcome::Return(Value::int(total as i64)))
    });
}

// ── File and stdio streams (not available on wasm) ───────────────

#[cfg(not(target_arch = "wasm32"))]
mod io_streams {
    use super::*;
    use std::io::{BufRead, BufReader, BufWriter, Read, Write};
    use std::rc::Rc;
    use std::time::Duration;

    use sema_core::cycle::GcEdge;
    use sema_core::runtime::{
        CompletionKind, NativeCallContext, NativeContinuation, NativeOutcome, NativeResult,
        NativeSuspend, ResourceGateHandle, ResourceGateId, ResumeInput, Trace, WaitKind,
    };
    use sema_core::{in_runtime_quantum, StreamBox};

    use crate::runtime_offload::{
        checkout_external, finish_terminal_gate, prepare_terminal_gate, suspend_terminal_external,
        CheckoutOp,
    };

    /// Completion-kind tag for file-stream `stream/*` external waits ("stf\0").
    const STREAM_COMPLETION_KIND: u64 = 0x7374_6600;

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
        /// Owning resource-gate capability minted on the first offloaded op.
        /// The stream object itself is the lifecycle owner; its `Drop` closes
        /// this gate when Sema code omits an explicit `stream/close`.
        gate: RefCell<Option<ResourceGateHandle>>,
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
                gate: RefCell::new(None),
            }
        }
    }

    impl Drop for FileInputStream {
        fn drop(&mut self) {
            if let Some(gate) = self.gate.get_mut().take() {
                let _ = gate.close();
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
        /// See [`FileInputStream::gate`].
        gate: RefCell<Option<ResourceGateHandle>>,
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
                gate: RefCell::new(None),
            }
        }
    }

    impl Drop for FileOutputStream {
        fn drop(&mut self) {
            if let Some(gate) = self.gate.get_mut().take() {
                let _ = gate.close();
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
    //
    // A file-backed stream owns one resource (a `BufReader`/`BufWriter<File>`)
    // that at most one offloaded op may hold at a time. Each op runs through the
    // CHECKOUT pattern under the unified runtime via
    // `crate::runtime_offload::checkout_external` (see `sqlite.rs`'s module doc
    // comment for the canonical writeup this mirrors): acquire the stream's
    // per-object [`ResourceGate`] (creating it on the first offload and caching
    // the id on the stream), take the resource out of the slot, run the blocking
    // op on the executor's blocking tier, then reinstall the resource and decode
    // on the VM thread before releasing the gate. A second `stream/*` op on a
    // busy stream parks FIFO on the gate; a mid-flight cancel tombstones the slot
    // (best-effort — the resource cannot be reclaimed). There is no process to
    // signal, so cancellation runs no abort.

    fn input_gate(stream: &Rc<StreamBox>) -> Option<ResourceGateHandle> {
        let inner = stream.borrow_inner();
        inner
            .as_any()
            .downcast_ref::<FileInputStream>()
            .and_then(|fis| fis.gate.borrow().clone())
    }

    fn store_input_gate(stream: &Rc<StreamBox>, gate: ResourceGateHandle) {
        let inner = stream.borrow_inner();
        if let Some(fis) = inner.as_any().downcast_ref::<FileInputStream>() {
            *fis.gate.borrow_mut() = Some(gate);
        }
    }

    fn remove_input_gate(stream: &Rc<StreamBox>, id: ResourceGateId) {
        let inner = stream.borrow_inner();
        if let Some(fis) = inner.as_any().downcast_ref::<FileInputStream>() {
            let mut gate = fis.gate.borrow_mut();
            if gate.as_ref().map(ResourceGateHandle::id) == Some(id) {
                gate.take();
            }
        }
    }

    fn output_gate(stream: &Rc<StreamBox>) -> Option<ResourceGateHandle> {
        let inner = stream.borrow_inner();
        inner
            .as_any()
            .downcast_ref::<FileOutputStream>()
            .and_then(|fos| fos.gate.borrow().clone())
    }

    fn store_output_gate(stream: &Rc<StreamBox>, gate: ResourceGateHandle) {
        let inner = stream.borrow_inner();
        if let Some(fos) = inner.as_any().downcast_ref::<FileOutputStream>() {
            *fos.gate.borrow_mut() = Some(gate);
        }
    }

    fn remove_output_gate(stream: &Rc<StreamBox>, id: ResourceGateId) {
        let inner = stream.borrow_inner();
        if let Some(fos) = inner.as_any().downcast_ref::<FileOutputStream>() {
            let mut gate = fos.gate.borrow_mut();
            if gate.as_ref().map(ResourceGateHandle::id) == Some(id) {
                gate.take();
            }
        }
    }

    fn gate_belongs_to_current_runtime(gate: &ResourceGateHandle) -> bool {
        sema_core::current_root().is_some_and(|root| root.runtime() == gate.id().runtime())
    }

    fn ensure_close_is_not_checked_out(stream: &Rc<StreamBox>) -> Result<(), SemaError> {
        let inner = stream.borrow_inner();
        let checked_out = match stream.stream_type() {
            "file-input" => inner
                .as_any()
                .downcast_ref::<FileInputStream>()
                .is_some_and(|stream| matches!(*stream.slot.borrow(), FileInSlot::CheckedOut)),
            "file-output" => inner
                .as_any()
                .downcast_ref::<FileOutputStream>()
                .is_some_and(|stream| matches!(*stream.slot.borrow(), FileOutSlot::CheckedOut)),
            _ => false,
        };
        if checked_out {
            Err(busy_err("stream/close"))
        } else {
            Ok(())
        }
    }

    fn close_foreign_input(stream: &Rc<StreamBox>) -> NativeResult {
        let tombstone = {
            let inner = stream.borrow_inner();
            let input = inner
                .as_any()
                .downcast_ref::<FileInputStream>()
                .expect("caller already verified stream_type() == \"file-input\"");
            let result = match &*input.slot.borrow() {
                FileInSlot::Tombstone(message) => Some(message.clone()),
                FileInSlot::Available(_) => None,
                FileInSlot::CheckedOut => unreachable!("busy state checked before owner close"),
            };
            result
        };
        if let Some(message) = tombstone {
            return Err(tombstone_err("stream/close", &message));
        }
        stream.close()?;
        Ok(NativeOutcome::Return(Value::nil()))
    }

    fn close_foreign_output(stream: &Rc<StreamBox>) -> NativeResult {
        let writer = take_output(stream, "stream/close")?;
        let kind = CompletionKind::try_from_raw(STREAM_COMPLETION_KIND)
            .expect("stream completion kind is nonzero");
        let stream_for_finish = stream.clone();
        let stream_for_tombstone = stream.clone();
        suspend_terminal_external(
            "stream/close",
            kind,
            writer,
            |writer| {
                writer
                    .flush()
                    .map_err(|error| render(format!("stream/close: I/O error: {error}")))
            },
            move |writer, result| {
                reinstall_output(&stream_for_finish, writer);
                result.map_err(SemaError::Io)?;
                stream_for_finish.close()?;
                Ok(Value::nil())
            },
            Rc::new(move |message| tombstone_output(&stream_for_tombstone, message)),
            None,
        )
    }

    /// Offload one blocking op against a file-INPUT stream's `BufReader` through
    /// the gate-guarded checkout. `op` runs off the VM thread; `decode` builds the
    /// final `Value` on the VM thread (fallible — e.g. `stream/copy` writes into
    /// the memory-side dst, which can itself reject).
    fn checkout_input<T: Send + 'static>(
        op_name: &'static str,
        stream: &Rc<StreamBox>,
        op: impl FnOnce(&mut BufReader<std::fs::File>) -> Result<T, String> + Send + 'static,
        decode: impl FnOnce(T) -> Result<Value, SemaError> + 'static,
    ) -> NativeResult {
        checkout_input_lifecycle(op_name, stream, op, decode, false)
    }

    fn checkout_input_lifecycle<T: Send + 'static>(
        op_name: &'static str,
        stream: &Rc<StreamBox>,
        op: impl FnOnce(&mut BufReader<std::fs::File>) -> Result<T, String> + Send + 'static,
        decode: impl FnOnce(T) -> Result<Value, SemaError> + 'static,
        terminal_on_success: bool,
    ) -> NativeResult {
        let kind = CompletionKind::try_from_raw(STREAM_COMPLETION_KIND)
            .expect("stream completion kind is nonzero");
        let gate = input_gate(stream);
        let s_store = stream.clone();
        let s_take = stream.clone();
        let s_remove = stream.clone();
        let s_reinstall = stream.clone();
        let s_tomb = stream.clone();
        checkout_external(CheckoutOp {
            op_name,
            kind,
            gate,
            store_gate: Box::new(move |id| store_input_gate(&s_store, id)),
            remove_gate: Rc::new(move |id| remove_input_gate(&s_remove, id)),
            take: Box::new(move || take_input(&s_take, op_name)),
            op: Box::new(op),
            reinstall: Box::new(move |r| reinstall_input(&s_reinstall, r)),
            decode: Box::new(decode),
            success_value: None,
            tombstone: Rc::new(move |msg| tombstone_input(&s_tomb, msg)),
            abort: None,
            terminal_on_success,
        })
    }

    /// Offload one blocking op against a file-OUTPUT stream's `BufWriter` through
    /// the gate-guarded checkout. See [`checkout_input`].
    fn checkout_output<T: Send + 'static>(
        op_name: &'static str,
        take_op: &'static str,
        stream: &Rc<StreamBox>,
        op: impl FnOnce(&mut BufWriter<std::fs::File>) -> Result<T, String> + Send + 'static,
        decode: impl FnOnce(T) -> Result<Value, SemaError> + 'static,
    ) -> NativeResult {
        let kind = CompletionKind::try_from_raw(STREAM_COMPLETION_KIND)
            .expect("stream completion kind is nonzero");
        let gate = output_gate(stream);
        let s_store = stream.clone();
        let s_take = stream.clone();
        let s_remove = stream.clone();
        let s_reinstall = stream.clone();
        let s_tomb = stream.clone();
        checkout_external(CheckoutOp {
            op_name,
            kind,
            gate,
            store_gate: Box::new(move |id| store_output_gate(&s_store, id)),
            remove_gate: Rc::new(move |id| remove_output_gate(&s_remove, id)),
            take: Box::new(move || take_output(&s_take, take_op)),
            op: Box::new(op),
            reinstall: Box::new(move |w| reinstall_output(&s_reinstall, w)),
            decode: Box::new(decode),
            success_value: None,
            tombstone: Rc::new(move |msg| tombstone_output(&s_tomb, msg)),
            abort: None,
            terminal_on_success: op_name == "stream/close",
        })
    }

    /// Take the file reader out of its slot once the gate is owned, marking the
    /// slot `CheckedOut`. With the gate held the slot must be `Available`; any
    /// other state (a cancelled prior op) is a clear domain error.
    fn take_input(stream: &Rc<StreamBox>, op: &str) -> Result<BufReader<std::fs::File>, SemaError> {
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
                Ok(r)
            }
            FileInSlot::CheckedOut => Err(busy_err(op)),
            FileInSlot::Tombstone(msg) => Err(tombstone_err(op, msg)),
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

    /// Take the file writer out of its slot once the gate is owned. A `Closed`
    /// slot (a copy targeting an already-closed dst, or the narrow concurrent-
    /// close race) is phrased exactly like `StreamBox::write`'s own message.
    fn take_output(
        stream: &Rc<StreamBox>,
        op: &str,
    ) -> Result<BufWriter<std::fs::File>, SemaError> {
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
                Ok(w)
            }
            FileOutSlot::CheckedOut => Err(busy_err(op)),
            FileOutSlot::Closed => Err(SemaError::eval(format!("{op}: stream is closed"))),
            FileOutSlot::Tombstone(msg) => Err(tombstone_err(op, msg)),
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
    ) -> Result<Option<NativeOutcome>, SemaError> {
        maybe_async_read_decoded(stream, n, Value::bytevector)
    }

    /// `stream/read-byte`'s async dispatch: same stdin/file-input offload
    /// shape as `maybe_async_read` (even the error text — `SemaStream::read`
    /// itself doesn't distinguish caller, so the sync path's error message
    /// for a failing `stream/read-byte` already reads "stream/read: ...")
    /// but with a 1-byte read and a byte-or-nil decode instead of a
    /// bytevector one.
    pub(super) fn maybe_async_read_byte(
        stream: &Rc<StreamBox>,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        maybe_async_read_decoded(stream, 1, |bytes| match bytes.first() {
            Some(&b) => Value::int(b as i64),
            None => Value::nil(),
        })
    }

    /// Shared offload body for `stream/read` and `stream/read-byte`: reads up
    /// to `n` bytes (stdin via `fs_offload`, file-input via the CHECKOUT
    /// pattern) and hands the raw bytes to `decode` to build the final
    /// `Value` on the VM thread once the offload completes.
    fn maybe_async_read_decoded(
        stream: &Rc<StreamBox>,
        n: usize,
        decode: fn(Vec<u8>) -> Value,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        if !in_runtime_quantum() {
            return Ok(None);
        }
        // *stdin* does real, potentially-blocking I/O (unlike in-memory byte
        // streams): offload the read so a waiting-for-input read can't stall the
        // cooperative scheduler. stdin is process-global and stateless, so we
        // read it directly on the worker instead of checking out the (VM-thread-
        // bound) stream box. Under the unified runtime this suspends structurally
        // on a quarantined-bounded External wait (the bound is a post-cancel
        // cleanup watchdog, never a running-read timeout, so a read blocked on
        // input is not faulted).
        if stream.stream_type() == "stdin" {
            return crate::io::quarantined_compute("stream/read", decode, move || {
                let mut buf = vec![0u8; n];
                let read = std::io::stdin()
                    .read(&mut buf)
                    .map_err(|e| render(format!("stream/read: stdin: {e}")))?;
                buf.truncate(read);
                Ok(buf)
            })
            .map(Some);
        }
        if stream.stream_type() != "file-input" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/read: stream is closed"));
        }
        Ok(Some(checkout_input(
            "stream/read",
            stream,
            move |reader: &mut BufReader<std::fs::File>| -> Result<Vec<u8>, String> {
                let mut buf = vec![0u8; n];
                let read = reader
                    .read(&mut buf)
                    .map_err(|e| render(format!("stream/read: I/O error: {e}")))?;
                buf.truncate(read);
                Ok(buf)
            },
            move |bytes: Vec<u8>| -> Result<Value, SemaError> { Ok(decode(bytes)) },
        )?))
    }

    /// One interruptible stdin aggregation. Stdin readiness is polled without
    /// blocking the VM thread; each ready chunk is read directly from the Unix
    /// fd and followed by a structural timer wake. The optional destination is
    /// a traced `Value`, keeping a `stream/copy` target alive across parks.
    struct StdinAggregation {
        bytes: Vec<u8>,
        cap: usize,
        destination: Option<Value>,
    }

    impl Trace for StdinAggregation {
        fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            if let Some(destination) = &self.destination {
                sink(GcEdge::Value(destination));
            }
            true
        }
    }

    impl StdinAggregation {
        fn op_name(&self) -> &'static str {
            if self.destination.is_some() {
                "stream/copy"
            } else {
                "stream/read-all"
            }
        }

        fn finish(mut self) -> NativeResult {
            let Some(destination) = self.destination.take() else {
                return Ok(NativeOutcome::Return(Value::bytevector(self.bytes)));
            };
            let destination = destination
                .as_stream_rc()
                .ok_or_else(|| SemaError::eval("stream/copy: destination stream was reclaimed"))?;
            if self.bytes.is_empty() {
                return Ok(NativeOutcome::Return(Value::int(0)));
            }
            let total = self.bytes.len();
            if destination.stream_type() == "file-output" {
                if destination.is_closed() {
                    return Err(SemaError::eval("stream/write: stream is closed"));
                }
                return checkout_output(
                    "stream/copy",
                    "stream/write",
                    &destination,
                    move |writer: &mut BufWriter<std::fs::File>| -> Result<(), String> {
                        writer
                            .write_all(&self.bytes)
                            .map_err(|e| render(format!("stream/copy: I/O error: {e}")))
                    },
                    move |()| Ok(Value::int(total as i64)),
                );
            }

            debug_assert_eq!(destination.stream_type(), "byte-buffer");
            destination.write(&self.bytes)?;
            Ok(NativeOutcome::Return(Value::int(total as i64)))
        }
    }

    struct StdinAggregationContinuation {
        state: Option<StdinAggregation>,
    }

    impl Trace for StdinAggregationContinuation {
        fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
            self.state.as_ref().is_none_or(|state| state.trace(sink))
        }
    }

    impl NativeContinuation for StdinAggregationContinuation {
        fn resume(
            mut self: Box<Self>,
            _context: &mut NativeCallContext<'_>,
            input: ResumeInput,
        ) -> NativeResult {
            let state = self
                .state
                .take()
                .expect("stdin aggregation continuation resumes once");
            match input {
                ResumeInput::Returned(_) => stdin_aggregation_step(state),
                ResumeInput::Failed(error) => Err(error),
                ResumeInput::Cancelled(reason) => Err(SemaError::eval(format!(
                    "{} was cancelled ({reason:?})",
                    state.op_name()
                ))),
                ResumeInput::Runtime(_) => Err(SemaError::eval(format!(
                    "{}: unexpected runtime response while polling stdin",
                    state.op_name()
                ))),
            }
        }
    }

    fn park_stdin_aggregation(state: StdinAggregation, delay: Duration) -> NativeResult {
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::Timer(delay),
            continuation: Box::new(StdinAggregationContinuation { state: Some(state) }),
        }))
    }

    #[cfg(unix)]
    fn read_ready_stdin(buf: &mut [u8], op: &str) -> Result<Option<usize>, SemaError> {
        // SAFETY: the fd set and timeval are initialized for a zero-timeout
        // readiness check, and `buf` provides a valid writable region to
        // `read(2)`. EINTR is reported as pending so the next structural wake
        // retries without blocking the VM thread.
        unsafe {
            let mut readfds: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut readfds);
            libc::FD_SET(libc::STDIN_FILENO, &mut readfds);
            let mut timeout = libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            };
            let ready = libc::select(
                libc::STDIN_FILENO + 1,
                &mut readfds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut timeout,
            );
            if ready == 0 {
                return Ok(None);
            }
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    return Ok(None);
                }
                return Err(SemaError::eval(format!(
                    "{op}: stdin readiness check failed: {error}"
                )));
            }

            let read = libc::read(
                libc::STDIN_FILENO,
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len(),
            );
            if read >= 0 {
                return Ok(Some(read as usize));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted
                || error.kind() == std::io::ErrorKind::WouldBlock
            {
                Ok(None)
            } else {
                Err(SemaError::eval(format!("{op}: stdin read failed: {error}")))
            }
        }
    }

    #[cfg(unix)]
    fn stdin_aggregation_step(mut state: StdinAggregation) -> NativeResult {
        // Read at most one byte beyond the remaining allowance. That byte is
        // enough to prove overflow while the aggregate buffer itself remains at
        // or below the cap.
        let read_len = capped_read_len(state.bytes.len(), state.cap);
        let mut chunk = [0u8; STREAM_CHUNK_BYTES];
        let Some(read) = read_ready_stdin(&mut chunk[..read_len], state.op_name())? else {
            return park_stdin_aggregation(state, Duration::from_millis(5));
        };
        if read == 0 {
            return state.finish();
        }
        let op = state.op_name();
        extend_aggregation(&mut state.bytes, &chunk[..read], state.cap, op)?;
        // A wake between every chunk makes a continuously-readable stdin
        // cooperatively interruptible and gives runnable siblings a turn.
        park_stdin_aggregation(state, Duration::from_millis(1))
    }

    #[cfg(not(unix))]
    fn stdin_aggregation_step(state: StdinAggregation) -> NativeResult {
        Err(SemaError::eval(format!(
            "{}: whole-stdin aggregation is unavailable in a runtime quantum on this platform",
            state.op_name()
        ))
        .with_hint("use stream/read in bounded chunks so each read remains cancellable"))
    }

    /// `stream/read-all`'s async dispatch. File input is checked out and read on
    /// a worker under the captured cap; stdin uses the interruptible readiness
    /// state machine above and never pins a worker while the pipe remains open.
    pub(super) fn maybe_async_read_all(
        stream: &Rc<StreamBox>,
        cap: usize,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        if !in_runtime_quantum() {
            return Ok(None);
        }
        if stream.stream_type() == "stdin" {
            return stdin_aggregation_step(StdinAggregation {
                bytes: Vec::new(),
                cap,
                destination: None,
            })
            .map(Some);
        }
        if stream.stream_type() != "file-input" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/read: stream is closed"));
        }
        Ok(Some(checkout_input(
            "stream/read-all",
            stream,
            move |reader: &mut BufReader<std::fs::File>| -> Result<Vec<u8>, String> {
                let mut result = Vec::new();
                let mut chunk = [0u8; STREAM_CHUNK_BYTES];
                loop {
                    let read_len = capped_read_len(result.len(), cap);
                    let n = reader
                        .read(&mut chunk[..read_len])
                        .map_err(|e| render(format!("stream/read: I/O error: {e}")))?;
                    if n == 0 {
                        break;
                    }
                    extend_aggregation(&mut result, &chunk[..n], cap, "stream/read-all")
                        .map_err(|error| render(error.to_string()))?;
                }
                Ok(result)
            },
            |bytes: Vec<u8>| -> Result<Value, SemaError> { Ok(Value::bytevector(bytes)) },
        )?))
    }

    pub(super) fn maybe_async_read_line(
        stream: &Rc<StreamBox>,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        if !in_runtime_quantum() {
            return Ok(None);
        }
        // *stdin* — see maybe_async_read: offload the blocking line read.
        if stream.stream_type() == "stdin" {
            fn decode_line(line: Option<String>) -> Value {
                match line {
                    None => Value::nil(),
                    Some(s) => Value::string(&s),
                }
            }
            return crate::io::quarantined_compute("stream/read-line", decode_line, move || {
                let mut line = String::new();
                let n = std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| render(format!("stream/read-line: stdin: {e}")))?;
                if n == 0 {
                    return Ok(None); // EOF, nothing read at all
                }
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                Ok(Some(line))
            })
            .map(Some);
        }
        if stream.stream_type() != "file-input" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/read-line: stream is closed"));
        }
        Ok(Some(checkout_input(
            "stream/read-line",
            stream,
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
        )?))
    }

    pub(super) fn maybe_async_write(
        stream: &Rc<StreamBox>,
        data: &[u8],
    ) -> Result<Option<NativeOutcome>, SemaError> {
        maybe_async_write_decoded(stream, data, |n| Ok(Value::int(n as i64)))
    }

    /// `stream/write-byte`'s async dispatch: same file-output CHECKOUT
    /// offload as `maybe_async_write`, but the decode always yields `nil` —
    /// matching the sync path, which (unlike `stream/write`) ignores the
    /// byte count `SemaStream::write` returns.
    pub(super) fn maybe_async_write_byte(
        stream: &Rc<StreamBox>,
        byte: u8,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        maybe_async_write_decoded(stream, &[byte], |_n| Ok(Value::nil()))
    }

    /// Shared offload body for `stream/write` and `stream/write-byte`.
    fn maybe_async_write_decoded(
        stream: &Rc<StreamBox>,
        data: &[u8],
        decode: impl FnOnce(usize) -> Result<Value, SemaError> + 'static,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        if !in_runtime_quantum() || stream.stream_type() != "file-output" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/write: stream is closed"));
        }
        let data = data.to_vec();
        Ok(Some(checkout_output(
            "stream/write",
            "stream/write",
            stream,
            move |writer: &mut BufWriter<std::fs::File>| -> Result<usize, String> {
                writer
                    .write(&data)
                    .map_err(|e| render(format!("stream/write: I/O error: {e}")))
            },
            decode,
        )?))
    }

    pub(super) fn maybe_async_flush(
        stream: &Rc<StreamBox>,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        if !in_runtime_quantum() || stream.stream_type() != "file-output" {
            return Ok(None);
        }
        if stream.is_closed() {
            return Err(SemaError::eval("stream/flush: stream is closed"));
        }
        Ok(Some(checkout_output(
            "stream/flush",
            "stream/flush",
            stream,
            move |writer: &mut BufWriter<std::fs::File>| -> Result<(), String> {
                writer
                    .flush()
                    .map_err(|e| render(format!("stream/flush: I/O error: {e}")))
            },
            |()| -> Result<Value, SemaError> { Ok(Value::nil()) },
        )?))
    }

    pub(super) fn maybe_async_close(
        stream: &Rc<StreamBox>,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        if stream.stream_type() != "file-input" && stream.stream_type() != "file-output" {
            return Ok(None);
        }
        if !in_runtime_quantum() {
            let (gate, remove): (Option<ResourceGateHandle>, Rc<dyn Fn(ResourceGateId)>) =
                match stream.stream_type() {
                    "file-input" => {
                        let remove_stream = stream.clone();
                        (
                            input_gate(stream),
                            Rc::new(move |id| remove_input_gate(&remove_stream, id)),
                        )
                    }
                    "file-output" => {
                        let remove_stream = stream.clone();
                        (
                            output_gate(stream),
                            Rc::new(move |id| remove_output_gate(&remove_stream, id)),
                        )
                    }
                    _ => return Ok(None),
                };
            if gate.is_none() {
                return Ok(None);
            }
            prepare_terminal_gate(gate.as_ref(), "stream/close")?;
            stream.close()?;
            return finish_terminal_gate(gate, remove, Ok(Value::nil())).map(Some);
        }

        let gate = match stream.stream_type() {
            "file-input" => input_gate(stream),
            "file-output" => output_gate(stream),
            _ => unreachable!("file stream type checked above"),
        };
        if let Some(gate) = gate
            .as_ref()
            .filter(|gate| !gate_belongs_to_current_runtime(gate))
        {
            ensure_close_is_not_checked_out(stream)?;
            prepare_terminal_gate(Some(gate), "stream/close")?;
            match stream.stream_type() {
                "file-input" => remove_input_gate(stream, gate.id()),
                "file-output" => remove_output_gate(stream, gate.id()),
                _ => unreachable!("file stream type checked above"),
            }
            if stream.is_closed() {
                return Ok(Some(NativeOutcome::Return(Value::nil())));
            }
            return match stream.stream_type() {
                "file-input" => close_foreign_input(stream).map(Some),
                "file-output" => close_foreign_output(stream).map(Some),
                _ => unreachable!("file stream type checked above"),
            };
        }
        if stream.stream_type() == "file-input" {
            if stream.is_closed() {
                let s_remove = stream.clone();
                return finish_terminal_gate(
                    input_gate(stream),
                    Rc::new(move |id| remove_input_gate(&s_remove, id)),
                    Ok(Value::nil()),
                )
                .map(Some);
            }
            let stream_for_finish = stream.clone();
            return Ok(Some(checkout_input_lifecycle(
                "stream/close",
                stream,
                |_reader| Ok(()),
                move |()| {
                    stream_for_finish.close()?;
                    Ok(Value::nil())
                },
                true,
            )?));
        }
        if stream.stream_type() != "file-output" {
            return Ok(None);
        }
        if stream.is_closed() {
            let s_remove = stream.clone();
            return finish_terminal_gate(
                output_gate(stream),
                Rc::new(move |id| remove_output_gate(&s_remove, id)),
                Ok(Value::nil()),
            )
            .map(Some);
        }
        let stream_for_finish = stream.clone();
        Ok(Some(checkout_output(
            "stream/close",
            "stream/close",
            stream,
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
        )?))
    }

    /// `stream/copy`'s async dispatch. A single file side is checked out and
    /// offloaded under the captured cap. Stdin uses cooperative readiness
    /// polling. Two file resources are rejected rather than entering a
    /// VM-thread EOF loop without ordered dual-gate acquisition.
    pub(super) fn maybe_async_copy(
        src: &Rc<StreamBox>,
        dst: &Rc<StreamBox>,
        cap: usize,
    ) -> Result<Option<NativeOutcome>, SemaError> {
        if !in_runtime_quantum() {
            return Ok(None);
        }

        if src.stream_type() == "stdin" {
            if dst.stream_type() != "file-output" && dst.stream_type() != "byte-buffer" {
                return Err(SemaError::eval(format!(
                    "stream/copy: stdin copy to a {} stream is unavailable in a runtime quantum",
                    dst.stream_type()
                ))
                .with_hint(
                    "copy stdin into a byte-buffer or file-output, or use stream/read and stream/write in bounded chunks",
                ));
            }
            return stdin_aggregation_step(StdinAggregation {
                bytes: Vec::new(),
                cap,
                destination: Some(Value::stream_from_rc(dst.clone())),
            })
            .map(Some);
        }

        let src_file = src.stream_type() == "file-input";
        let dst_file = dst.stream_type() == "file-output";
        if !src_file && !dst_file {
            return Ok(None);
        }
        if src_file && dst_file {
            return Err(SemaError::eval(
                "stream/copy: file-to-file copy is unavailable inside a runtime quantum; copy with stream/read and stream/write in bounded chunks",
            )
            .with_hint(
                "ordered dual-resource acquisition is required for a one-call file-to-file copy",
            ));
        }

        if src_file {
            if src.is_closed() {
                return Err(SemaError::eval("stream/read: stream is closed"));
            }
            let dst_for_decode = dst.clone();
            return Ok(Some(checkout_input(
                "stream/copy",
                src,
                move |reader: &mut BufReader<std::fs::File>| -> Result<Vec<u8>, String> {
                    let mut out = Vec::new();
                    let mut chunk = [0u8; STREAM_CHUNK_BYTES];
                    loop {
                        let read_len = capped_read_len(out.len(), cap);
                        let n = reader
                            .read(&mut chunk[..read_len])
                            .map_err(|e| render(format!("stream/copy: I/O error: {e}")))?;
                        if n == 0 {
                            break;
                        }
                        extend_aggregation(&mut out, &chunk[..n], cap, "stream/copy")
                            .map_err(|error| render(error.to_string()))?;
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
            )?));
        }

        // In-memory source: reading is a fast in-process copy (no real I/O), so
        // build a bounded snapshot on the VM thread, then offload the file write.
        let mut buf = Vec::new();
        let mut chunk = [0u8; STREAM_CHUNK_BYTES];
        loop {
            let read_len = capped_read_len(buf.len(), cap);
            let n = src.read(&mut chunk[..read_len])?;
            if n == 0 {
                break;
            }
            extend_aggregation(&mut buf, &chunk[..n], cap, "stream/copy")?;
        }
        if buf.is_empty() {
            // Nothing read — the sync loop would never have touched dst
            // either, so there's nothing to offload.
            return Ok(Some(NativeOutcome::Return(Value::int(0))));
        }
        let total = buf.len();
        Ok(Some(checkout_output(
            "stream/copy",
            "stream/write",
            dst,
            move |writer: &mut BufWriter<std::fs::File>| -> Result<(), String> {
                writer
                    .write_all(&buf)
                    .map_err(|e| render(format!("stream/copy: I/O error: {e}")))
            },
            move |()| -> Result<Value, SemaError> { Ok(Value::int(total as i64)) },
        )?))
    }

    /// Decode a worker-opened `BufReader` into a file-input stream `Value` on the
    /// VM thread. A plain `fn` for [`crate::io::quarantined_compute`]'s decoder.
    fn input_stream_value(reader: BufReader<std::fs::File>) -> Value {
        Value::stream(FileInputStream::from_reader(reader))
    }

    /// Decode a worker-opened `BufWriter` into a file-output stream `Value`.
    fn output_stream_value(writer: BufWriter<std::fs::File>) -> Value {
        Value::stream(FileOutputStream::from_writer(writer))
    }

    /// `stream/open-input`'s dispatch: under the unified runtime the blocking
    /// `File::open` suspends structurally on a quarantined-bounded External wait —
    /// mirrors `db/open`, there is no existing stream to contend over. Sync stays
    /// today's shape.
    pub(super) fn open_input(path: &str) -> NativeResult {
        if in_runtime_quantum() {
            let path = path.to_string();
            return crate::io::quarantined_compute(
                "stream/open-input",
                input_stream_value,
                move || {
                    std::fs::File::open(&path)
                        .map(BufReader::new)
                        .map_err(|e| render(format!("stream/open-input: {path}: {e}")))
                },
            );
        }
        Ok(NativeOutcome::Return(Value::stream(FileInputStream::open(
            path,
        )?)))
    }

    /// `stream/open-output`'s dispatch — see `open_input`.
    pub(super) fn open_output(path: &str) -> NativeResult {
        if in_runtime_quantum() {
            let path = path.to_string();
            return crate::io::quarantined_compute(
                "stream/open-output",
                output_stream_value,
                move || {
                    std::fs::File::create(&path)
                        .map(BufWriter::new)
                        .map_err(|e| render(format!("stream/open-output: {path}: {e}")))
                },
            );
        }
        Ok(NativeOutcome::Return(Value::stream(
            FileOutputStream::create(path)?,
        )))
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

    crate::register_runtime_fn_path_gated(
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

    crate::register_runtime_fn_path_gated(
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
