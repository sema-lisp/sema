//! Serial port primitives (`serial/*`).
//!
//! Ports live in a thread-local registry keyed by an incrementing handle ID
//! (`u64`). `Box<dyn serialport::SerialPort>` is `Send` (the trait itself
//! requires it — asserted below), so a call that blocks on real I/O —
//! opening the device, or reading/writing bytes over the wire — can move the
//! port onto the I/O pool's blocking tier and back instead of blocking the
//! VM thread for the operation's whole duration. Inside an `async/spawn`'d
//! task that would otherwise stall every sibling on the cooperative
//! scheduler.
//!
//! `serial/open` offloads the device `open()` syscall itself via
//! `fs_offload` (io.rs): there is no existing port to contend over, so the
//! poller simply inserts the freshly-opened, freshly-`BufReader`-wrapped
//! port into the registry on completion — mirrors `db/open`'s shape
//! (`sqlite.rs`).
//!
//! `serial/write`/`serial/read-line`/`serial/send` run against an EXISTING
//! port, so they use the CHECKOUT pattern (see `sqlite.rs`'s module doc
//! comment for the canonical writeup this mirrors): the registry slot is
//! `Available(Port)` / `CheckedOut` / `Tombstone(msg)`. The offload takes the
//! port out of the slot for its duration; any other `serial/*` op on the
//! SAME handle sees `CheckedOut` and either errors clearly (the sync path,
//! and `serial/close`) or queues (an async caller's `IoHandle` re-attempts
//! the checkout every poll — the `Acquire` phase — until the slot frees up,
//! then runs its own offload). The offload's poller reinstalls the port as
//! `Available` and calls `notify_io_complete()` so a sibling queued on the
//! same handle can't miss the wakeup.
//!
//! `serial/write`'s `flush()` is included in its offload — on most serialport
//! backends flush maps to `tcdrain(3)`, which blocks until every queued byte
//! has actually been transmitted, not just handed to the kernel — so it's as
//! blocking as the write itself. `serial/send` offloads the entire
//! write+flush+read-line round trip as one op (matching the sync path, which
//! never yields control between the write and the response read); JSON
//! parsing of the response happens inside the offload too (`serde_json::Value`
//! is plain `Send` data, not a Sema `Value`), with only the final
//! `serde_json::Value -> Value` conversion happening in `decode` on the VM
//! thread.
//!
//! At top level (no scheduler) every builtin keeps today's synchronous shape
//! byte-for-byte.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use sema_core::{check_arity, in_async_context, Caps, IoHandle, IoPoll, SemaError, Value};

/// A registry entry: a buffered handle over a boxed trait object port.
type Port = BufReader<Box<dyn serialport::SerialPort>>;

// `Port` moves across the offload boundary (every checkout op). This
// compiles only if it stays `Send`; `serialport::SerialPort: Send` is a
// trait-level requirement, but a future change to this module's port type
// fails here, not with an opaque trait-bound error deep in
// `sema_io::io_spawn_blocking`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Port>();
};

/// A registry slot. `CheckedOut` is the moment between an offload taking the
/// port out and the poller reinstalling it; every other `serial/*` op treats
/// it as "busy, try again once the in-flight op resolves". `Tombstone` is
/// terminal: set only when an offload is cancelled mid-flight (the port is
/// stuck inside an uncancellable background thread — see `spawn_port_op`'s
/// doc comment) or its worker vanishes unexpectedly; `serial/close` is the
/// only way to free a tombstoned slot.
enum PortSlot {
    Available(Port),
    CheckedOut,
    Tombstone(String),
}

// Thread-local serial port storage, keyed by an incrementing handle ID.
thread_local! {
    static PORTS: RefCell<HashMap<u64, PortSlot>> = RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u64> = const { RefCell::new(1) };
}

fn next_handle() -> u64 {
    NEXT_ID.with(|id| {
        let h = *id.borrow();
        *id.borrow_mut() = h + 1;
        h
    })
}

/// `handle` has never been `serial/open`ed (or was already `serial/close`d).
/// Text matches the pre-offload message verbatim (`"{op}: invalid handle
/// {handle}"`) — every sync-path call site rendered it this way.
fn missing_err(op: &str, handle: u64) -> SemaError {
    SemaError::eval(format!("{op}: invalid handle {handle}"))
}

/// `op` was attempted while an offload had `handle` checked out.
fn busy_err(op: &str, handle: u64) -> SemaError {
    SemaError::eval(format!(
        "{op}: serial port {handle} is busy — another serial/* call is in flight on it"
    ))
    .with_hint(
        "wait for the in-flight serial/* call on this handle to resolve before calling another",
    )
}

/// `op` was attempted on a handle whose in-flight offload was cancelled.
fn tombstone_err(op: &str, handle: u64, reason: &str) -> SemaError {
    SemaError::eval(format!(
        "{op}: serial port {handle} is no longer usable: {reason}"
    ))
}

/// Pre-render `op: {e}` through the same `SemaError::eval` constructor the
/// sync path raises, so the message text an async rejection carries is
/// substring-identical to what the sync path would display for the same
/// failure (mirrors `eval_msg` in sqlite.rs/kv.rs).
fn eval_msg(op: &str, e: impl std::fmt::Display) -> String {
    SemaError::eval(format!("{op}: {e}")).to_string()
}

/// Sync-path / non-offloaded accessor: look up `handle` for an op that only
/// needs `&mut Port`, translating the other slot states into a clear,
/// `op`-specific error instead of ever panicking on the enum shape. Used by
/// every offloadable op's OWN top-level (non-async) branch.
fn with_port<R>(
    op: &str,
    handle: u64,
    f: impl FnOnce(&mut Port) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    PORTS.with(|ports| {
        let mut ports = ports.borrow_mut();
        match ports.get_mut(&handle) {
            Some(PortSlot::Available(port)) => f(port),
            Some(PortSlot::CheckedOut) => Err(busy_err(op, handle)),
            Some(PortSlot::Tombstone(msg)) => Err(tombstone_err(op, handle, msg)),
            None => Err(missing_err(op, handle)),
        }
    })
}

/// What crosses the thread boundary from an offloaded port op back to the
/// poller: the reinstalled `Port` plus the op's owned `Send` result. Mirrors
/// `sqlite.rs`'s `ConnOpOutcome`.
struct PortOpOutcome<T> {
    port: Port,
    result: Result<T, String>,
}

/// The two phases a checkout offload's `IoHandle` cycles through — identical
/// shape to `sqlite.rs`'s `ConnPhase`. A caller that finds the slot
/// immediately `Available` still starts in `Acquire`; it succeeds on the
/// very first poll and falls through into `Running` in the same tick, so
/// there is exactly one code path for both the uncontended and the queued
/// case.
enum PortPhase<T> {
    /// Waiting for the slot to become `Available`. Re-checked every poll;
    /// never mutates anything beyond that check, so aborting here is a true
    /// no-op — nothing was ever taken out.
    Acquire,
    /// Holding the checkout; `op` is running on the I/O pool. Resolves with
    /// the reinstalled `Port` plus the op's result.
    Running(tokio::sync::oneshot::Receiver<PortOpOutcome<T>>),
}

/// Move `op` on `port` onto the I/O pool's blocking tier. Cancellation past
/// this point is best-effort by construction (the `Port` is inside a
/// `spawn_blocking` closure with no abort hook — the same tradeoff every
/// other `spawn_blocking`-based offload in this codebase accepts, see
/// `IoHandle::with_abort`'s doc comment): the caller marks the registry slot
/// `Tombstone` on abort so a later access errors clearly instead of the slot
/// staying `CheckedOut` forever with no one left to reinstall it, but the
/// blocking call itself keeps running unattended on the worker.
fn spawn_port_op<T: Send + 'static>(
    mut port: Port,
    op: impl FnOnce(&mut Port) -> Result<T, String> + Send + 'static,
) -> tokio::sync::oneshot::Receiver<PortOpOutcome<T>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let result = op(&mut port);
        let _ = tx.send(PortOpOutcome { port, result });
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });
    rx
}

/// Offload one blocking serial operation on the port named `handle` through
/// the CHECKOUT pattern (see the module doc comment). `op` runs on the I/O
/// pool; `decode` turns its owned `Send` result into the final `Value` on
/// the VM thread when the scheduler polls the completed offload — mirrors
/// `sqlite.rs`'s `checkout_offload`. Returns `Ok(nil)` after arming the
/// yield signal; the scheduler delivers the real value on resume.
fn checkout_offload<T: Send + 'static>(
    op_name: &'static str,
    handle: u64,
    op: impl FnOnce(&mut Port) -> Result<T, String> + Send + 'static,
    decode: impl Fn(T) -> Value + 'static,
) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let phase = Rc::new(RefCell::new(PortPhase::<T>::Acquire));
    let phase_for_poll = phase.clone();
    let mut op_holder = Some(op);

    let poll = move || -> IoPoll {
        loop {
            let is_acquire = matches!(&*phase_for_poll.borrow(), PortPhase::Acquire);
            if is_acquire {
                // Owned Result so the PORTS borrow doesn't outlive the match
                // — the `Running` transition below needs its own
                // (non-overlapping) borrow of the same thread-local.
                let mut taken: Option<Result<Port, String>> = None;
                PORTS.with(|p| {
                    let mut ports = p.borrow_mut();
                    match ports.get_mut(&handle) {
                        Some(slot @ PortSlot::Available(_)) => {
                            let PortSlot::Available(port) =
                                std::mem::replace(slot, PortSlot::CheckedOut)
                            else {
                                unreachable!("just matched Available")
                            };
                            taken = Some(Ok(port));
                        }
                        Some(PortSlot::CheckedOut) => {}
                        Some(PortSlot::Tombstone(msg)) => {
                            taken = Some(Err(tombstone_err(op_name, handle, msg).to_string()));
                        }
                        None => {
                            taken = Some(Err(missing_err(op_name, handle).to_string()));
                        }
                    }
                });
                match taken {
                    None => return IoPoll::Pending,
                    Some(Err(msg)) => return IoPoll::Ready(Err(msg)),
                    Some(Ok(port)) => {
                        let op = op_holder
                            .take()
                            .expect("checkout_offload's op is consumed exactly once");
                        *phase_for_poll.borrow_mut() = PortPhase::Running(spawn_port_op(port, op));
                        // Fall through: poll the freshly spawned receiver
                        // immediately instead of wasting a scheduler tick.
                    }
                }
            } else {
                let mut phase_ref = phase_for_poll.borrow_mut();
                let PortPhase::Running(rx) = &mut *phase_ref else {
                    unreachable!("Acquire handled above")
                };
                return match rx.try_recv() {
                    Err(TryRecvError::Empty) => IoPoll::Pending,
                    Ok(outcome) => {
                        drop(phase_ref);
                        PORTS.with(|p| {
                            p.borrow_mut()
                                .insert(handle, PortSlot::Available(outcome.port))
                        });
                        // MANDATORY lost-wakeup guard: a sibling queued on this
                        // same handle (still in `Acquire`) may have polled
                        // Pending earlier in this scheduler sweep — without
                        // this it would park until an unrelated wakeup.
                        sema_core::notify_io_complete();
                        match outcome.result {
                            Ok(t) => IoPoll::Ready(Ok(decode(t))),
                            Err(msg) => IoPoll::Ready(Err(msg)),
                        }
                    }
                    Err(TryRecvError::Closed) => {
                        drop(phase_ref);
                        PORTS.with(|p| {
                            p.borrow_mut().insert(
                                handle,
                                PortSlot::Tombstone(
                                    "the I/O worker terminated unexpectedly".to_string(),
                                ),
                            )
                        });
                        IoPoll::Ready(Err(format!("{op_name}: I/O worker dropped")))
                    }
                };
            }
        }
    };

    let phase_for_abort = phase.clone();
    let io_handle = Rc::new(IoHandle::with_abort(poll, move || {
        // Acquire-phase abort: no-op — nothing was ever checked out, the
        // registry slot is exactly as another caller left it. Running-phase
        // abort: best-effort — see `spawn_port_op`'s doc comment.
        if matches!(*phase_for_abort.borrow(), PortPhase::Running(_)) {
            PORTS.with(|p| {
                p.borrow_mut().insert(
                    handle,
                    PortSlot::Tombstone(format!(
                        "{op_name} was cancelled while in flight; the port cannot be \
                         reclaimed — serial/close frees the handle"
                    )),
                );
            });
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(io_handle));
    Ok(Value::nil())
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (serial/list) => list of available port names
    crate::register_fn_gated(env, sandbox, Caps::SERIAL, "serial/list", |args| {
        check_arity!(args, "serial/list", 0);
        let ports = serialport::available_ports()
            .map_err(|e| SemaError::eval(format!("serial/list: {e}")))?;
        let names: Vec<Value> = ports.iter().map(|p| Value::string(&p.port_name)).collect();
        Ok(Value::list(names))
    });

    // (serial/open path baud) => handle (int)
    // (serial/open path baud timeout_ms) => handle (int)
    crate::register_fn_gated(env, sandbox, Caps::SERIAL, "serial/open", |args| {
        if args.len() < 2 || args.len() > 3 {
            return Err(SemaError::arity("serial/open", "2-3", args.len()));
        }
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let baud = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?
            as u32;
        let timeout_ms = if args.len() == 3 {
            args[2]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[2].type_name()))?
                as u64
        } else {
            2000
        };

        if in_async_context() {
            let path_for_open = path.clone();
            return crate::io::fs_offload(
                move || {
                    serialport::new(&path_for_open, baud)
                        .timeout(Duration::from_millis(timeout_ms))
                        .open()
                        .map_err(|e| {
                            SemaError::eval(format!("serial/open: {e}"))
                                .with_hint(format!("path={path_for_open}, baud={baud}"))
                                .to_string()
                        })
                },
                move |port| {
                    let handle = next_handle();
                    let reader = BufReader::new(port);
                    PORTS.with(|ports| {
                        ports
                            .borrow_mut()
                            .insert(handle, PortSlot::Available(reader))
                    });
                    Value::int(handle as i64)
                },
            );
        }

        let port = serialport::new(&path, baud)
            .timeout(Duration::from_millis(timeout_ms))
            .open()
            .map_err(|e| {
                SemaError::eval(format!("serial/open: {e}"))
                    .with_hint(format!("path={path}, baud={baud}"))
            })?;

        let handle = next_handle();
        let reader = BufReader::new(port);
        PORTS.with(|ports| {
            ports
                .borrow_mut()
                .insert(handle, PortSlot::Available(reader))
        });
        Ok(Value::int(handle as i64))
    });

    // (serial/close handle) => nil
    //
    // A handle checked out by an in-flight offload errors instead of racing
    // the background op for the same `Port` (matches `db/close`/`kv/close`);
    // a tombstoned handle is a silent no-op removal — `serial/close` remains
    // the documented way to free either. A missing handle keeps the original
    // synchronous error text verbatim.
    crate::register_fn_gated(env, sandbox, Caps::SERIAL, "serial/close", |args| {
        check_arity!(args, "serial/close", 1);
        let handle = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            as u64;
        PORTS.with(|ports| {
            let mut ports = ports.borrow_mut();
            match ports.get(&handle) {
                Some(PortSlot::CheckedOut) => Err(busy_err("serial/close", handle)),
                Some(_) => {
                    ports.remove(&handle);
                    Ok(Value::nil())
                }
                None => Err(SemaError::eval(format!(
                    "serial/close: invalid handle {handle}"
                ))),
            }
        })
    });

    // (serial/write handle string) => nil
    crate::register_fn_gated(env, sandbox, Caps::SERIAL, "serial/write", |args| {
        check_arity!(args, "serial/write", 2);
        let handle = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            as u64;
        let data = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        if in_async_context() {
            return checkout_offload(
                "serial/write",
                handle,
                move |reader| {
                    let port = reader.get_mut();
                    port.write_all(data.as_bytes())
                        .map_err(|e| eval_msg("serial/write", e))?;
                    port.flush()
                        .map_err(|e| eval_msg("serial/write flush", e))?;
                    Ok(())
                },
                |()| Value::nil(),
            );
        }

        with_port("serial/write", handle, |reader| {
            let port = reader.get_mut();
            port.write_all(data.as_bytes())
                .map_err(|e| SemaError::eval(format!("serial/write: {e}")))?;
            port.flush()
                .map_err(|e| SemaError::eval(format!("serial/write flush: {e}")))?;
            Ok(Value::nil())
        })
    });

    // (serial/read-line handle) => string (reads until \n)
    crate::register_fn_gated(env, sandbox, Caps::SERIAL, "serial/read-line", |args| {
        check_arity!(args, "serial/read-line", 1);
        let handle = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            as u64;

        if in_async_context() {
            return checkout_offload(
                "serial/read-line",
                handle,
                move |reader| {
                    let mut line = String::new();
                    reader
                        .read_line(&mut line)
                        .map_err(|e| eval_msg("serial/read-line", e))?;
                    Ok(line)
                },
                |line: String| {
                    // Trim trailing \r\n
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    Value::string(trimmed)
                },
            );
        }

        with_port("serial/read-line", handle, |reader| {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| SemaError::eval(format!("serial/read-line: {e}")))?;
            // Trim trailing \r\n
            let trimmed = line.trim_end_matches(['\r', '\n']);
            Ok(Value::string(trimmed))
        })
    });

    // (serial/send handle command) => parsed JSON response
    // Sends command + \n, reads one line back, parses as JSON.
    // Convenience for the sema-bridge protocol.
    crate::register_fn_gated(env, sandbox, Caps::SERIAL, "serial/send", |args| {
        check_arity!(args, "serial/send", 2);
        let handle = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            as u64;
        let cmd = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        if in_async_context() {
            return checkout_offload(
                "serial/send",
                handle,
                move |reader| {
                    // Write command + newline
                    let port = reader.get_mut();
                    port.write_all(cmd.as_bytes())
                        .map_err(|e| eval_msg("serial/send write", e))?;
                    port.write_all(b"\n")
                        .map_err(|e| eval_msg("serial/send write", e))?;
                    port.flush().map_err(|e| eval_msg("serial/send flush", e))?;

                    // Read response line
                    let mut line = String::new();
                    reader
                        .read_line(&mut line)
                        .map_err(|e| eval_msg("serial/send read", e))?;

                    // Parse JSON response (plain Send data — no Sema Value
                    // touched on this worker thread; the final conversion to
                    // a Sema Value happens in `decode`, on the VM thread).
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return Ok(None);
                    }
                    let json_val: serde_json::Value = serde_json::from_str(trimmed)
                        .map_err(|e| eval_msg("serial/send parse", format!("{e}: {trimmed}")))?;
                    Ok(Some(json_val))
                },
                |json_val: Option<serde_json::Value>| match json_val {
                    Some(v) => sema_core::json::json_to_value(&v),
                    None => Value::nil(),
                },
            );
        }

        with_port("serial/send", handle, |reader| {
            // Write command + newline
            let port = reader.get_mut();
            port.write_all(cmd.as_bytes())
                .map_err(|e| SemaError::eval(format!("serial/send write: {e}")))?;
            port.write_all(b"\n")
                .map_err(|e| SemaError::eval(format!("serial/send write: {e}")))?;
            port.flush()
                .map_err(|e| SemaError::eval(format!("serial/send flush: {e}")))?;

            // Read response line
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| SemaError::eval(format!("serial/send read: {e}")))?;

            // Parse JSON response
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return Ok(Value::nil());
            }
            let json_val: serde_json::Value = serde_json::from_str(trimmed)
                .map_err(|e| SemaError::eval(format!("serial/send parse: {e}: {trimmed}")))?;
            Ok(sema_core::json::json_to_value(&json_val))
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> sema_core::Env {
        let e = sema_core::Env::new();
        register(&e, &sema_core::Sandbox::allow_all());
        e
    }

    fn native(env: &sema_core::Env, name: &str) -> impl Fn(&[Value]) -> Result<Value, SemaError> {
        let f = env
            .get(sema_core::intern(name))
            .unwrap_or_else(|| panic!("{name} not registered"));
        move |args: &[Value]| {
            let nf = f.as_native_fn_ref().expect("native fn");
            let ctx = sema_core::EvalContext::new();
            (nf.func)(&ctx, args)
        }
    }

    /// Forces `in_async_context()` on for the guard's lifetime, resetting it
    /// (even on panic/early return) so a failure can't leak the flag into
    /// whichever test the harness runs next on the same worker thread —
    /// mirrors io.rs's `AsyncCtxGuard`.
    struct AsyncCtxGuard;
    impl Drop for AsyncCtxGuard {
        fn drop(&mut self) {
            sema_core::set_async_context(false);
        }
    }

    /// Call a native fn with the async-context gate forced on, then drive the
    /// `AwaitIo` handle it arms to completion by polling. Panics if the
    /// native didn't yield at all (e.g. it silently took the sync fallback).
    /// Returns the raw rejection string on failure — NOT re-wrapped through
    /// `SemaError::eval` — because the string an `IoPoll::Ready(Err(_))`
    /// carries is already pre-rendered (via `eval_msg`/`missing_err(...).
    /// to_string()`) to be substring-identical to the sync path's
    /// `SemaError::eval(...).to_string()`; wrapping it again would double
    /// the "Eval error: " prefix.
    fn drive_async(call: impl FnOnce() -> Result<Value, SemaError>) -> Result<Value, String> {
        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let armed = call().map_err(|e| e.to_string())?;
        assert_eq!(
            armed,
            Value::nil(),
            "an offloading native returns nil immediately after arming its yield signal"
        );
        let reason = sema_core::take_yield_signal()
            .expect("expected a yield signal to be armed — did the native take the sync path?");
        let handle = match reason {
            sema_core::YieldReason::AwaitIo(h) => h,
            other => panic!("expected an AwaitIo yield, got {other:?}"),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match handle.poll() {
                IoPoll::Ready(Ok(v)) => return Ok(v),
                IoPoll::Ready(Err(e)) => return Err(e),
                IoPoll::Pending => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "offload never completed within 10s"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
    }

    // No real serial hardware is available in this environment, so these
    // tests exercise the parts of the scheduler-offload gate that don't
    // require an actually-open port: the sync path stays byte-for-byte
    // identical, and the async path's checkout correctly reports "missing
    // handle" (the `Acquire` phase's `None` branch, exercised without ever
    // spawning a blocking worker) instead of silently doing nothing.

    #[test]
    fn read_line_sync_path_missing_handle_unchanged() {
        let e = env();
        let err = native(&e, "serial/read-line")(&[Value::int(999)]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Eval error: serial/read-line: invalid handle 999"
        );
    }

    #[test]
    fn read_line_async_path_missing_handle_matches_sync_text() {
        let e = env();
        let err = drive_async(|| native(&e, "serial/read-line")(&[Value::int(999)])).unwrap_err();
        assert_eq!(err, "Eval error: serial/read-line: invalid handle 999");
    }

    #[test]
    fn write_sync_path_missing_handle_unchanged() {
        let e = env();
        let err = native(&e, "serial/write")(&[Value::int(999), Value::string("hi")]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Eval error: serial/write: invalid handle 999"
        );
    }

    #[test]
    fn write_async_path_missing_handle_matches_sync_text() {
        let e = env();
        let err =
            drive_async(|| native(&e, "serial/write")(&[Value::int(999), Value::string("hi")]))
                .unwrap_err();
        assert_eq!(err, "Eval error: serial/write: invalid handle 999");
    }

    #[test]
    fn send_sync_path_missing_handle_unchanged() {
        let e = env();
        let err = native(&e, "serial/send")(&[Value::int(999), Value::string("ping")]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Eval error: serial/send: invalid handle 999"
        );
    }

    #[test]
    fn send_async_path_missing_handle_matches_sync_text() {
        let e = env();
        let err =
            drive_async(|| native(&e, "serial/send")(&[Value::int(999), Value::string("ping")]))
                .unwrap_err();
        assert_eq!(err, "Eval error: serial/send: invalid handle 999");
    }

    #[test]
    fn close_missing_handle_errors_in_both_modes() {
        let e = env();
        let err = native(&e, "serial/close")(&[Value::int(999)]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Eval error: serial/close: invalid handle 999"
        );
    }

    #[test]
    fn open_async_path_errors_cleanly_on_bad_device() {
        // No real hardware available; opening a nonexistent device path must
        // still round-trip cleanly through fs_offload and reject with the
        // same message shape the sync path would raise.
        let e = env();
        let result = drive_async(|| {
            native(&e, "serial/open")(&[
                Value::string("/dev/sema-nonexistent-test-device"),
                Value::int(9600),
            ])
        });
        assert!(result.is_err(), "opening a nonexistent device should fail");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("serial/open"),
            "error should mention serial/open: {msg}"
        );
    }
}
