//! Pseudo-terminal primitives (`pty/*`).
//!
//! Like `proc/*`, but the child runs under a real PTY, so programs that probe
//! `isatty` (REPLs, `vim`, `top`, anything with color/line-editing) behave as if
//! attached to a terminal. stdout+stderr are merged onto the pty master and
//! drained by a reader thread into a pollable buffer; `pty/resize` propagates
//! window-size changes (SIGWINCH). Handles are integer ids into a thread-local
//! registry. Use `pty/close` to free a handle.
//!
//! `pty/wait` blocks on `Child::wait()`, which can run for the child's whole
//! lifetime. Inside an `async/spawn`'d task that would stall every sibling on
//! the cooperative scheduler, so it offloads through the same CHECKOUT design
//! `proc/wait` uses (see `proc.rs`'s module doc comment): the registry slot
//! (`PtySlot`) is `Available(Pty)` / `CheckedOut` / `Tombstone(reason)`.
//! `pty/wait` takes the `Pty` (it is `Send` — see the static assertion below)
//! out of the slot for the offload's duration; every other `pty/*` op sees
//! `CheckedOut` and errors clearly. The offload's poller reinstalls the `Pty`
//! and calls `notify_io_complete()` so a queued sibling `pty/wait` on the same
//! handle can't miss the wakeup. At top level the sync path is unchanged.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use sema_core::{check_arity, in_async_context, Caps, IoHandle, IoPoll, SemaError, Value};

struct Pty {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    out: Arc<Mutex<Vec<u8>>>,
    reader_thread: Option<JoinHandle<()>>,
}

// `pty/wait`'s offload moves a whole `Pty` onto the I/O pool's blocking tier
// and back. This compiles only if every field stays `Send`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Pty>();
};

/// A registry slot — see `proc.rs`'s `ProcSlot` for the full design rationale
/// (identical here, just for `Pty`).
enum PtySlot {
    Available(Pty),
    CheckedOut,
    Tombstone(String),
}

thread_local! {
    static PTYS: RefCell<HashMap<i64, PtySlot>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<i64> = const { Cell::new(1) };
}

fn pump(mut reader: Box<dyn Read + Send>, buf: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
    })
}

fn handle(args: &[Value], idx: usize) -> Result<i64, SemaError> {
    args[idx]
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer (pty handle)", args[idx].type_name()))
}

fn u16_opt(m: &std::collections::BTreeMap<Value, Value>, key: &str, default: u16) -> u16 {
    m.get(&Value::keyword(key))
        .and_then(|v| v.as_int())
        .map(|n| n.clamp(1, u16::MAX as i64) as u16)
        .unwrap_or(default)
}

fn missing_err(op: &str, id: i64) -> SemaError {
    SemaError::eval(format!("{op}: no such handle {id}"))
}

/// `op` was attempted while `pty/wait` had this handle checked out.
fn busy_err(op: &str, id: i64) -> SemaError {
    SemaError::eval(format!(
        "{op}: handle {id} is busy — a pty/wait is in flight on it"
    ))
    .with_hint(
        "wait for the in-flight pty/wait to resolve before calling another pty/* op on this handle",
    )
}

/// `op` was attempted on a handle whose in-flight `pty/wait` was cancelled.
fn tombstone_err(op: &str, id: i64, reason: &str) -> SemaError {
    SemaError::eval(format!("{op}: handle {id} is no longer usable: {reason}"))
}

/// Look up `id` for an op that needs `&mut Pty` — see `proc.rs`'s `with_proc`.
fn with_pty<R>(
    op: &str,
    id: i64,
    f: impl FnOnce(&mut Pty) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    PTYS.with(|p| {
        let mut ptys = p.borrow_mut();
        match ptys.get_mut(&id) {
            Some(PtySlot::Available(pt)) => f(pt),
            Some(PtySlot::CheckedOut) => Err(busy_err(op, id)),
            Some(PtySlot::Tombstone(msg)) => Err(tombstone_err(op, id, msg)),
            None => Err(missing_err(op, id)),
        }
    })
}

fn spawn(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "pty/spawn", 1..=2);
    let argv = args[0]
        .as_list()
        .or_else(|| args[0].as_vector())
        .ok_or_else(|| SemaError::type_error("list of strings (argv)", args[0].type_name()))?;
    if argv.is_empty() {
        return Err(SemaError::eval("pty/spawn: argv must be non-empty"));
    }
    let mut parts: Vec<String> = Vec::with_capacity(argv.len());
    for v in argv {
        parts.push(
            v.as_str()
                .ok_or_else(|| SemaError::type_error("string", v.type_name()))?
                .to_string(),
        );
    }

    let opts = args.get(1).and_then(|o| o.as_map_ref());
    let (rows, cols) = match opts {
        Some(m) => (u16_opt(m, "rows", 24), u16_opt(m, "cols", 80)),
        None => (24, 80),
    };

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| SemaError::eval(format!("pty/spawn: {e}")))?;

    let mut cmd = CommandBuilder::new(&parts[0]);
    for a in &parts[1..] {
        cmd.arg(a);
    }
    if let Some(m) = opts {
        if let Some(cwd) = m.get(&Value::keyword("cwd")).and_then(|v| v.as_str()) {
            cmd.cwd(cwd);
        }
        if let Some(em) = m.get(&Value::keyword("env")).and_then(|v| v.as_map_ref()) {
            for (k, val) in em.iter() {
                if let (Some(k), Some(val)) = (k.as_str(), val.as_str()) {
                    cmd.env(k, val);
                }
            }
        }
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| SemaError::eval(format!("pty/spawn {}: {e}", parts[0])))?;
    // Drop the slave so the master read sees EOF once the child exits.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| SemaError::eval(format!("pty/spawn: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| SemaError::eval(format!("pty/spawn: {e}")))?;
    let out = Arc::new(Mutex::new(Vec::new()));
    let reader_thread = Some(pump(reader, out.clone()));

    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    PTYS.with(|p| {
        p.borrow_mut().insert(
            id,
            PtySlot::Available(Pty {
                master: pair.master,
                child,
                writer,
                out,
                reader_thread,
            }),
        )
    });
    Ok(Value::int(id))
}

/// What crosses the thread boundary from the offloaded `child.wait()` back to
/// the poller — see `proc.rs`'s `WaitOutcome`.
struct WaitOutcome {
    pty: Pty,
    status: Result<i32, String>,
}

/// Move `pty`'s blocking `child.wait()` — plus joining the reader thread —
/// onto the I/O pool's blocking tier. Best-effort cancellation only, same
/// tradeoff as `proc.rs`'s `spawn_proc_wait` (see its doc comment).
fn spawn_pty_wait(mut pty: Pty) -> tokio::sync::oneshot::Receiver<WaitOutcome> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let status = pty
            .child
            .wait()
            .map(|s| s.exit_code() as i32)
            .map_err(|e| format!("pty/wait: {e}"));
        if let Some(t) = pty.reader_thread.take() {
            let _ = t.join();
        }
        let _ = tx.send(WaitOutcome { pty, status });
        sema_core::notify_io_complete();
    });
    rx
}

/// The two phases a `pty/wait` `IoHandle` cycles through — mirrors `proc.rs`'s
/// `WaitPhase`.
enum WaitPhase {
    Acquire,
    Running(tokio::sync::oneshot::Receiver<WaitOutcome>),
}

fn poll_wait(id: i64, phase: &mut WaitPhase) -> IoPoll {
    use tokio::sync::oneshot::error::TryRecvError;
    loop {
        match phase {
            WaitPhase::Acquire => {
                enum Acquired {
                    Not,
                    Pty(Pty),
                    Err(String),
                }
                let acquired = PTYS.with(|p| {
                    let mut ptys = p.borrow_mut();
                    match ptys.get_mut(&id) {
                        Some(slot @ PtySlot::Available(_)) => {
                            let PtySlot::Available(pt) =
                                std::mem::replace(slot, PtySlot::CheckedOut)
                            else {
                                unreachable!("just matched Available")
                            };
                            Acquired::Pty(pt)
                        }
                        Some(PtySlot::CheckedOut) => Acquired::Not,
                        Some(PtySlot::Tombstone(msg)) => {
                            Acquired::Err(tombstone_err("pty/wait", id, msg).to_string())
                        }
                        None => Acquired::Err(missing_err("pty/wait", id).to_string()),
                    }
                });
                match acquired {
                    Acquired::Not => return IoPoll::Pending,
                    Acquired::Err(msg) => return IoPoll::Ready(Err(msg)),
                    Acquired::Pty(pt) => {
                        *phase = WaitPhase::Running(spawn_pty_wait(pt));
                    }
                }
            }
            WaitPhase::Running(rx) => {
                return match rx.try_recv() {
                    Err(TryRecvError::Empty) => IoPoll::Pending,
                    Ok(outcome) => {
                        PTYS.with(|p| p.borrow_mut().insert(id, PtySlot::Available(outcome.pty)));
                        sema_core::notify_io_complete();
                        match outcome.status {
                            Ok(code) => IoPoll::Ready(Ok(Value::int(code as i64))),
                            Err(msg) => IoPoll::Ready(Err(SemaError::Io(msg).to_string())),
                        }
                    }
                    Err(TryRecvError::Closed) => {
                        PTYS.with(|p| {
                            p.borrow_mut().insert(
                                id,
                                PtySlot::Tombstone(
                                    "the wait worker terminated unexpectedly".to_string(),
                                ),
                            )
                        });
                        IoPoll::Ready(Err("pty/wait: subprocess wait worker dropped".to_string()))
                    }
                };
            }
        }
    }
}

/// The async-context `pty/wait` entry point — mirrors `proc.rs`'s
/// `proc_wait_async`.
fn pty_wait_async(id: i64) -> Result<Value, SemaError> {
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let phase = Rc::new(RefCell::new(WaitPhase::Acquire));
    let phase_for_poll = phase.clone();
    let handle = Rc::new(IoHandle::with_abort(
        move || poll_wait(id, &mut phase_for_poll.borrow_mut()),
        move || {
            if matches!(*phase.borrow(), WaitPhase::Running(_)) {
                PTYS.with(|p| {
                    p.borrow_mut().insert(
                        id,
                        PtySlot::Tombstone(
                            "pty/wait was cancelled while the wait was in flight; the \
                             process may still be running in the background but this \
                             handle can no longer reach it — pty/close frees the slot; \
                             there is no way to reconnect"
                                .to_string(),
                        ),
                    );
                });
            }
        },
    ));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(handle));
    Ok(Value::nil())
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/spawn", spawn);

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/read", |args| {
        check_arity!(args, "pty/read", 1);
        let id = handle(args, 0)?;
        with_pty("pty/read", id, |pt| {
            let mut b = pt.out.lock().unwrap_or_else(|e| e.into_inner());
            let s = String::from_utf8_lossy(&b).into_owned();
            b.clear();
            Ok(Value::string(&s))
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/write", |args| {
        check_arity!(args, "pty/write", 2);
        let id = handle(args, 0)?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        with_pty("pty/write", id, |pt| {
            pt.writer
                .write_all(text.as_bytes())
                .and_then(|_| pt.writer.flush())
                .map_err(|e| SemaError::Io(format!("pty/write: {e}")))?;
            Ok(Value::nil())
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/resize", |args| {
        check_arity!(args, "pty/resize", 3);
        let id = handle(args, 0)?;
        let rows = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[1].type_name()))?
            .clamp(1, u16::MAX as i64) as u16;
        let cols = args[2]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[2].type_name()))?
            .clamp(1, u16::MAX as i64) as u16;
        with_pty("pty/resize", id, |pt| {
            pt.master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| SemaError::eval(format!("pty/resize: {e}")))?;
            Ok(Value::nil())
        })
    });

    // pty/wait — block until exit, return the exit code. Async context
    // offloads via `pty_wait_async`; top level keeps the original synchronous
    // shape byte-for-byte — see `proc/wait`'s doc comment (identical design).
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/wait", |args| {
        check_arity!(args, "pty/wait", 1);
        let id = handle(args, 0)?;
        if in_async_context() {
            return pty_wait_async(id);
        }
        let mut pt = PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            match ptys.remove(&id) {
                Some(PtySlot::Available(pt)) => Ok(pt),
                Some(slot @ PtySlot::CheckedOut) => {
                    ptys.insert(id, slot);
                    Err(busy_err("pty/wait", id))
                }
                Some(PtySlot::Tombstone(msg)) => Err(tombstone_err("pty/wait", id, &msg)),
                None => Err(missing_err("pty/wait", id)),
            }
        })?;
        let status = pt.child.wait();
        if let Some(t) = pt.reader_thread.take() {
            let _ = t.join();
        }
        PTYS.with(|p| p.borrow_mut().insert(id, PtySlot::Available(pt)));
        let status = status.map_err(|e| SemaError::Io(format!("pty/wait: {e}")))?;
        Ok(Value::int(status.exit_code() as i64))
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/exit-code", |args| {
        check_arity!(args, "pty/exit-code", 1);
        let id = handle(args, 0)?;
        with_pty("pty/exit-code", id, |pt| {
            match pt
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("pty/exit-code: {e}")))?
            {
                Some(status) => Ok(Value::int(status.exit_code() as i64)),
                None => Ok(Value::nil()),
            }
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/running?", |args| {
        check_arity!(args, "pty/running?", 1);
        let id = handle(args, 0)?;
        with_pty("pty/running?", id, |pt| {
            let running = pt
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("pty/running?: {e}")))?
                .is_none();
            Ok(Value::bool(running))
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/kill", |args| {
        check_arity!(args, "pty/kill", 1);
        let id = handle(args, 0)?;
        with_pty("pty/kill", id, |pt| {
            let _ = pt.child.kill();
            Ok(Value::nil())
        })
    });

    // pty/close — see `proc/close`'s doc comment (identical design).
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/close", |args| {
        check_arity!(args, "pty/close", 1);
        let id = handle(args, 0)?;
        PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            if matches!(ptys.get(&id), Some(PtySlot::CheckedOut)) {
                return Err(busy_err("pty/close", id));
            }
            if let Some(PtySlot::Available(mut pt)) = ptys.remove(&id) {
                let _ = pt.child.kill();
                let _ = pt.child.wait();
            }
            Ok(Value::nil())
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{EvalContext, Sandbox};

    fn env() -> sema_core::Env {
        let e = sema_core::Env::new();
        register(&e, &Sandbox::allow_all());
        e
    }

    fn call(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let f = env.get_str(name).expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        (nf.func)(&EvalContext::default(), args).expect("call ok")
    }

    #[test]
    fn pty_runs_a_command() {
        let e = env();
        let h = call(
            &e,
            "pty/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("printf hi"),
            ])],
        );
        let code = call(&e, "pty/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        let out = call(&e, "pty/read", &[h.clone()]);
        // PTY output may carry CR/LF translation; just assert our text is present.
        assert!(out.as_str().unwrap().contains("hi"));
        call(&e, "pty/close", &[h]);
    }

    #[test]
    fn isatty_is_true_under_pty() {
        let e = env();
        let h = call(
            &e,
            "pty/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("test -t 1 && printf TTY || printf NOTTY"),
            ])],
        );
        call(&e, "pty/wait", &[h.clone()]);
        let out = call(&e, "pty/read", &[h.clone()]);
        assert!(out.as_str().unwrap().contains("TTY"));
        call(&e, "pty/close", &[h]);
    }

    /// Today's behavior (sync path): `pty/wait` may be called more than once
    /// on the same handle and returns the identical exit code each time — see
    /// `proc.rs`'s `double_wait_returns_same_code_sync` (same underlying
    /// `std::process::Child::wait` cache, since portable-pty's unix `Child`
    /// impl delegates to it directly). The async path
    /// (proc_pty_async_test.rs) asserts this stays true through the offload.
    #[test]
    fn double_wait_returns_same_code_sync() {
        let e = env();
        let h = call(
            &e,
            "pty/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("exit 7"),
            ])],
        );
        let first = call(&e, "pty/wait", &[h.clone()]);
        let second = call(&e, "pty/wait", &[h.clone()]);
        assert_eq!(first.as_int(), Some(7));
        assert_eq!(second.as_int(), Some(7));
        call(&e, "pty/close", &[h]);
    }
}
