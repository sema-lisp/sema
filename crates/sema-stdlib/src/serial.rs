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
//! `serial/open` offloads the device `open()` syscall itself as a plain
//! External wait (`runtime_offload::external_io_interruptible_try`): there is
//! no existing port to contend over, so the decoder simply inserts the
//! freshly-opened, freshly-`BufReader`-wrapped port into the registry on
//! completion — mirrors `db/open`'s shape (`sqlite.rs`).
//!
//! `serial/write`/`serial/read-line`/`serial/send` run against an EXISTING
//! port, so they use the CHECKOUT pattern under the unified runtime via
//! `runtime_offload::checkout_external` (see `sqlite.rs`'s module doc comment
//! for the canonical writeup this mirrors): the registry slot is
//! `Available(Port)` / `CheckedOut` / `Tombstone(msg)`, guarded by a per-handle
//! `ResourceGate` that serializes concurrent ops FIFO. The offload acquires the
//! gate, takes the port out of the slot, runs the blocking op on the executor's
//! blocking tier, then reinstalls the port and releases the gate. Any other
//! `serial/*` op on the SAME handle either errors clearly (the sync path, and
//! `serial/close`, on `CheckedOut`) or parks FIFO on the gate. A mid-flight
//! cancel tombstones the slot (best-effort — the port cannot be reclaimed) and
//! releases the gate; there is no process to signal, so no abort runs.
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
use std::rc::Rc;
use std::time::Duration;

use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult, ResourceGateId};
use sema_core::{check_arity, in_runtime_quantum, Caps, SemaError, Value};

use crate::runtime_offload::{checkout_external, CheckoutOp};

/// Completion-kind tag for `serial/*` external waits ("srl\0").
const SERIAL_COMPLETION_KIND: u64 = 0x7372_6c00;

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
    /// Per-handle [`ResourceGateId`], created lazily on the first offloaded op on
    /// a handle and reused for its later ops (dropped on `serial/close`). The gate
    /// provides FIFO mutual exclusion for the checkout slot.
    static SERIAL_GATES: RefCell<HashMap<u64, ResourceGateId>> = RefCell::new(HashMap::new());
}

/// Take `handle`'s port out of its slot once its gate is owned, marking the slot
/// `CheckedOut`. A tombstoned/missing/busy slot fails with the same clear text
/// the sync path raises.
fn take_port(op: &'static str, handle: u64) -> Result<Port, SemaError> {
    PORTS.with(|p| {
        let mut ports = p.borrow_mut();
        match ports.get_mut(&handle) {
            Some(slot @ PortSlot::Available(_)) => {
                let PortSlot::Available(port) = std::mem::replace(slot, PortSlot::CheckedOut)
                else {
                    unreachable!("just matched Available")
                };
                Ok(port)
            }
            Some(PortSlot::CheckedOut) => Err(busy_err(op, handle)),
            Some(PortSlot::Tombstone(msg)) => Err(tombstone_err(op, handle, msg)),
            None => Err(missing_err(op, handle)),
        }
    })
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

/// Offload one blocking serial operation on the port `handle` through the
/// CHECKOUT pattern under the unified runtime (see `sqlite.rs`'s module doc
/// comment for the canonical writeup this mirrors): acquire the handle's
/// [`ResourceGate`] (creating it on first use), take the `Port` out of its slot,
/// run `op` off the VM thread on the executor's blocking tier, then reinstall the
/// `Port` and decode the result on the VM thread before releasing the gate. A
/// second `serial/*` op on a busy handle parks FIFO on the gate; a mid-flight
/// cancel tombstones the slot (best-effort — the blocking call keeps running
/// unattended, so the port cannot be reclaimed) and releases the gate. There is
/// no process to signal, so — unlike proc/pty — cancellation runs no abort.
fn checkout_runtime<T: Send + 'static>(
    op_name: &'static str,
    handle: u64,
    op: impl FnOnce(&mut Port) -> Result<T, String> + Send + 'static,
    decode: impl FnOnce(T) -> Value + 'static,
) -> NativeResult {
    let kind = CompletionKind::try_from_raw(SERIAL_COMPLETION_KIND)
        .expect("serial completion kind is nonzero");
    let gate = SERIAL_GATES.with(|g| g.borrow().get(&handle).copied());
    checkout_external(CheckoutOp {
        op_name,
        kind,
        gate,
        store_gate: Box::new(move |gid| {
            SERIAL_GATES.with(|g| {
                g.borrow_mut().insert(handle, gid);
            });
        }),
        take: Box::new(move || take_port(op_name, handle)),
        op: Box::new(op),
        reinstall: Box::new(move |port| {
            PORTS.with(|p| {
                p.borrow_mut().insert(handle, PortSlot::Available(port));
            });
        }),
        decode: Box::new(move |t| Ok(decode(t))),
        success_value: None,
        tombstone: Rc::new(move |msg| {
            PORTS.with(|p| {
                p.borrow_mut().insert(handle, PortSlot::Tombstone(msg));
            });
        }),
        abort: None,
    })
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
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::SERIAL, "serial/open", &[], |args| {
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

        // There is no existing port to contend over, so `serial/open` offloads
        // the blocking device `open()` as a plain External wait (mirrors
        // `db/open`'s shape): the decoder inserts the freshly-`BufReader`-wrapped
        // port into the registry on completion.
        if in_runtime_quantum() {
            let kind = CompletionKind::try_from_raw(SERIAL_COMPLETION_KIND)
                .expect("serial completion kind is nonzero");
            let path_for_open = path;
            return crate::runtime_offload::external_io_interruptible_try(
                "serial/open",
                kind,
                "serial/open",
                move |port: Box<dyn serialport::SerialPort>| {
                    let handle = next_handle();
                    let reader = BufReader::new(port);
                    PORTS.with(|ports| {
                        ports
                            .borrow_mut()
                            .insert(handle, PortSlot::Available(reader))
                    });
                    Ok(Value::int(handle as i64))
                },
                move || async move {
                    serialport::new(&path_for_open, baud)
                        .timeout(Duration::from_millis(timeout_ms))
                        .open()
                        .map_err(|e| {
                            SemaError::eval(format!("serial/open: {e}"))
                                .with_hint(format!("path={path_for_open}, baud={baud}"))
                                .to_string()
                        })
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
        Ok(NativeOutcome::Return(Value::int(handle as i64)))
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
                    Ok(())
                }
                None => Err(SemaError::eval(format!(
                    "serial/close: invalid handle {handle}"
                ))),
            }
        })?;
        // The handle's resource gate is dropped here too; a successful close
        // implies the gate is idle (a busy gate means CheckedOut, which errors
        // above), so no waiter is stranded.
        SERIAL_GATES.with(|g| {
            g.borrow_mut().remove(&handle);
        });
        Ok(Value::nil())
    });

    // (serial/write handle string) => nil
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::SERIAL,
        "serial/write",
        &[],
        |args| {
            check_arity!(args, "serial/write", 2);
            let handle = args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
                as u64;
            let data = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                .to_string();

            if in_runtime_quantum() {
                return checkout_runtime(
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
            .map(NativeOutcome::Return)
        },
    );

    // (serial/read-line handle) => string (reads until \n)
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::SERIAL,
        "serial/read-line",
        &[],
        |args| {
            check_arity!(args, "serial/read-line", 1);
            let handle = args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
                as u64;

            if in_runtime_quantum() {
                return checkout_runtime(
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
            .map(NativeOutcome::Return)
        },
    );

    // (serial/send handle command) => parsed JSON response
    // Sends command + \n, reads one line back, parses as JSON.
    // Convenience for the sema-bridge protocol.
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::SERIAL, "serial/send", &[], |args| {
        check_arity!(args, "serial/send", 2);
        let handle = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?
            as u64;
        let cmd = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();

        if in_runtime_quantum() {
            return checkout_runtime(
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
        .map(NativeOutcome::Return)
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

    // No real serial hardware is available in this environment, so these tests
    // exercise the sync path: it stays byte-for-byte identical after the
    // unified-runtime conversion. The async checkout path (missing-handle text +
    // cancellation) is covered by `crates/sema/tests/serial_async_test.rs`,
    // which drives the real cooperative runtime.

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
    fn write_sync_path_missing_handle_unchanged() {
        let e = env();
        let err = native(&e, "serial/write")(&[Value::int(999), Value::string("hi")]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Eval error: serial/write: invalid handle 999"
        );
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
    fn close_missing_handle_errors_in_both_modes() {
        let e = env();
        let err = native(&e, "serial/close")(&[Value::int(999)]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Eval error: serial/close: invalid handle 999"
        );
    }
}
