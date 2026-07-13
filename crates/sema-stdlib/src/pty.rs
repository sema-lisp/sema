//! Pseudo-terminal primitives (`pty/*`).
//!
//! Like `proc/*`, but the child runs under a real PTY, so programs that probe
//! `isatty` (REPLs, `vim`, `top`, anything with color/line-editing) behave as if
//! attached to a terminal. stdout+stderr are merged onto the pty master and
//! drained by a reader thread into a pollable buffer; `pty/resize` propagates
//! window-size changes (SIGWINCH). Handles are integer ids into a thread-local
//! registry. Use `pty/close` to free a handle.
//!
//! `pty/wait`, `pty/write`, and `pty/close` each block on the OS (`Child::wait()`,
//! a possibly-backpressured `Write::write_all`, or `kill()` + `wait()`), any of
//! which can run for a long time. Inside an `async/spawn`'d task that would stall
//! every sibling on the cooperative scheduler, so all three offload through the
//! same CHECKOUT design `proc/wait`/`proc/write-stdin`/`proc/close` use (see
//! `proc.rs`'s module doc comment): the registry slot (`PtySlot`) is
//! `Available(Pty)` / `CheckedOut` / `Tombstone(reason)`. Each offload takes the
//! `Pty` (it is `Send` — see the static assertion below) out of the slot for its
//! duration; every other `pty/*` op sees `CheckedOut` and errors clearly. The
//! offload's poller reinstalls the `Pty` (or, for `pty/close`, drops it after
//! reaping) and calls `notify_io_complete()` so a queued sibling op on the same
//! handle can't miss the wakeup. At top level every sync path is unchanged.

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

/// What crosses the thread boundary from the offloaded `writer.write_all` +
/// `flush` back to the poller: the reinstalled `Pty` plus the write outcome.
/// Only `Send` data ever crosses — the bytes to write are copied into an
/// owned `Vec<u8>` before the offload starts, never a `Value`. Mirrors
/// `proc.rs`'s `WriteOutcome`/`spawn_proc_write_stdin`.
struct WriteOutcome {
    pty: Pty,
    result: Result<(), String>,
}

/// Move `pty`'s blocking `writer.write_all(text) + flush()` onto the I/O
/// pool's blocking tier — a child that isn't draining the pty's input side
/// (or a slow reader on the master) can block a write indefinitely, same
/// tradeoff as `spawn_pty_wait`. `text` is a plain owned `Vec<u8>` so the
/// closure stays `Send + 'static`.
fn spawn_pty_write(mut pty: Pty, text: Vec<u8>) -> tokio::sync::oneshot::Receiver<WriteOutcome> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let result = pty
            .writer
            .write_all(&text)
            .and_then(|_| pty.writer.flush())
            .map_err(|e| format!("pty/write: {e}"));
        let _ = tx.send(WriteOutcome { pty, result });
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });
    rx
}

/// The two phases a `pty/write` `IoHandle` cycles through — same
/// Acquire/Running shape as `WaitPhase` (see `poll_wait`), except `Acquire`
/// also carries the pending bytes so they can be moved into the offload
/// exactly once, on the transition into `Running`.
enum WritePhase {
    Acquire(Vec<u8>),
    Running(tokio::sync::oneshot::Receiver<WriteOutcome>),
}

/// Poll (and drive) one `pty/write`'s `Acquire` → `Running` state machine.
/// Mirrors `poll_wait` field-for-field (see `proc.rs`'s `poll_write_stdin`).
fn poll_write(id: i64, phase: &mut WritePhase) -> IoPoll {
    use tokio::sync::oneshot::error::TryRecvError;
    loop {
        match phase {
            WritePhase::Acquire(_) => {
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
                            Acquired::Err(tombstone_err("pty/write", id, msg).to_string())
                        }
                        None => Acquired::Err(missing_err("pty/write", id).to_string()),
                    }
                });
                match acquired {
                    Acquired::Not => return IoPoll::Pending,
                    Acquired::Err(msg) => return IoPoll::Ready(Err(msg)),
                    Acquired::Pty(pt) => {
                        let WritePhase::Acquire(text) = phase else {
                            unreachable!("just matched Acquire")
                        };
                        let text = std::mem::take(text);
                        *phase = WritePhase::Running(spawn_pty_write(pt, text));
                        // Fall through: poll the freshly spawned receiver
                        // immediately instead of wasting a scheduler tick.
                    }
                }
            }
            WritePhase::Running(rx) => {
                return match rx.try_recv() {
                    Err(TryRecvError::Empty) => IoPoll::Pending,
                    Ok(outcome) => {
                        PTYS.with(|p| p.borrow_mut().insert(id, PtySlot::Available(outcome.pty)));
                        // MANDATORY lost-wakeup guard: a sibling queued on this
                        // same handle (still in `Acquire`) may have polled
                        // Pending earlier in this scheduler sweep — without
                        // this it would park until an unrelated wakeup.
                        sema_core::notify_io_complete();
                        match outcome.result {
                            Ok(()) => IoPoll::Ready(Ok(Value::nil())),
                            Err(msg) => IoPoll::Ready(Err(SemaError::Io(msg).to_string())),
                        }
                    }
                    Err(TryRecvError::Closed) => {
                        PTYS.with(|p| {
                            p.borrow_mut().insert(
                                id,
                                PtySlot::Tombstone(
                                    "the write worker terminated unexpectedly".to_string(),
                                ),
                            )
                        });
                        IoPoll::Ready(Err("pty/write: write worker dropped".to_string()))
                    }
                };
            }
        }
    }
}

/// The async-context `pty/write` entry point: yields `AwaitIo` and lets the
/// scheduler drive `poll_write` to completion instead of blocking the VM
/// thread on `writer.write_all` (pty backpressure from a slow/stuck child
/// would otherwise stall every sibling on the cooperative scheduler).
fn pty_write_async(id: i64, text: Vec<u8>) -> Result<Value, SemaError> {
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let phase = Rc::new(RefCell::new(WritePhase::Acquire(text)));
    let phase_for_poll = phase.clone();
    let handle = Rc::new(IoHandle::with_abort(
        move || poll_write(id, &mut phase_for_poll.borrow_mut()),
        move || {
            // Acquire-phase abort: no-op, nothing was checked out yet.
            // Running-phase abort: best-effort, same tradeoff as
            // `pty/wait`'s abort (see `spawn_pty_wait`'s doc comment) — the
            // write may still be landing on the worker with no way to
            // interrupt it.
            if matches!(*phase.borrow(), WritePhase::Running(_)) {
                PTYS.with(|p| {
                    p.borrow_mut().insert(
                        id,
                        PtySlot::Tombstone(
                            "pty/write was cancelled while the write was in flight; the \
                             write may have partially landed but this handle can no \
                             longer reach it — pty/close frees the slot"
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

/// The state `pty/close`'s async offload passes through while its
/// `spawn_pty_wait` offload (started right after the synchronous, non-
/// blocking `kill()`) runs on the I/O pool. Unlike `pty/wait`'s `WaitPhase`,
/// there is no `Acquire` retry here: `pty/close` never queues behind a busy
/// handle — a handle a `pty/wait`/`pty/write` already has checked out is a
/// hard, immediate `busy_err`, exactly like the sync path. Mirrors
/// `proc.rs`'s `ClosePhase`.
struct ClosePhase(tokio::sync::oneshot::Receiver<WaitOutcome>);

/// Poll `pty/close`'s in-flight `spawn_pty_wait` offload to completion.
fn poll_close(id: i64, phase: &mut ClosePhase) -> IoPoll {
    use tokio::sync::oneshot::error::TryRecvError;
    match phase.0.try_recv() {
        Err(TryRecvError::Empty) => IoPoll::Pending,
        Ok(_outcome) => {
            // Unlike `pty/wait`, `pty/close` frees the slot instead of
            // reinstalling it — the reaped `Pty` (reader thread already
            // joined inside the offload) is simply dropped here, exactly
            // like the sync path drops it after `ptys.remove(&id)`.
            PTYS.with(|p| {
                p.borrow_mut().remove(&id);
            });
            sema_core::notify_io_complete();
            IoPoll::Ready(Ok(Value::nil()))
        }
        Err(TryRecvError::Closed) => {
            PTYS.with(|p| {
                p.borrow_mut().insert(
                    id,
                    PtySlot::Tombstone("the close worker terminated unexpectedly".to_string()),
                )
            });
            IoPoll::Ready(Err("pty/close: wait worker dropped".to_string()))
        }
    }
}

/// The async-context `pty/close` entry point. Mirrors the sync path exactly
/// up through `kill()` — checked out synchronously on the VM thread (`kill`
/// is a signal send, not a wait, so this stays cheap), `busy_err` on a
/// handle a `pty/wait`/`pty/write` already has checked out, silent no-op on
/// a `Tombstone`/missing handle — then offloads only the blocking
/// `child.wait()` + reader-thread join, reusing `spawn_pty_wait` exactly as
/// `pty/wait` does instead of duplicating it.
fn pty_close_async(id: i64) -> Result<Value, SemaError> {
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let taken: Option<Pty> = PTYS.with(|p| -> Result<Option<Pty>, SemaError> {
        let mut ptys = p.borrow_mut();
        if matches!(ptys.get(&id), Some(PtySlot::CheckedOut)) {
            return Err(busy_err("pty/close", id));
        }
        Ok(match ptys.remove(&id) {
            Some(PtySlot::Available(pt)) => Some(pt),
            // Tombstone or missing: already unusable/gone — the sync path
            // treats both as a silent no-op via the same unconditional
            // `ptys.remove(&id)`, so does this one.
            _ => None,
        })
    })?;

    let Some(mut pt) = taken else {
        return Ok(Value::nil());
    };

    let _ = pt.child.kill(); // a signal send — cheap and non-blocking, not a wait

    // Mark the slot busy for the offloaded wait+join's duration so a
    // concurrent pty/* op on this id sees a clear "busy" error instead of
    // racing the reader-thread join or seeing "missing handle" mid-reap.
    PTYS.with(|p| {
        p.borrow_mut().insert(id, PtySlot::CheckedOut);
    });

    let phase = Rc::new(RefCell::new(ClosePhase(spawn_pty_wait(pt))));
    let phase_for_poll = phase.clone();
    let handle = Rc::new(IoHandle::with_abort(
        move || poll_close(id, &mut phase_for_poll.borrow_mut()),
        move || {
            // Best-effort, same tradeoff as `pty/wait`'s abort (see
            // `spawn_pty_wait`'s doc comment): the child is already killed,
            // but its wait+join keeps running unattended inside
            // `spawn_blocking` with no abort hook, so the slot is
            // tombstoned rather than left `CheckedOut` forever.
            PTYS.with(|p| {
                p.borrow_mut().insert(
                    id,
                    PtySlot::Tombstone(
                        "pty/close was cancelled while reaping the killed process; the \
                         process was already killed but this handle can no longer reach \
                         it"
                        .to_string(),
                    ),
                );
            });
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
        if in_async_context() {
            return pty_write_async(id, text.as_bytes().to_vec());
        }
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

    // pty/close — see `proc/close`'s doc comment (identical design). Async
    // context offloads via `pty_close_async`; top level keeps the original
    // synchronous shape byte-for-byte.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "pty/close", |args| {
        check_arity!(args, "pty/close", 1);
        let id = handle(args, 0)?;
        if in_async_context() {
            return pty_close_async(id);
        }
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

/// Async-context coverage for the `pty/write` / `pty/close` scheduler
/// offloads added to this file — mirrors `proc.rs`'s `async_offload_tests`
/// module byte-for-byte in structure (see its doc comment for why the
/// scheduler is simulated by hand here instead of going through
/// `sema-eval`).
#[cfg(test)]
mod async_offload_tests {
    use super::*;
    use sema_core::{EvalContext, Sandbox};
    use std::time::{Duration, Instant};

    /// Forces `in_async_context()` on for the guard's lifetime, resetting it
    /// (even on panic/early return) so a failure can't leak the flag into
    /// whichever test the harness runs next on the same worker thread —
    /// mirrors `proc.rs`'s `AsyncCtxGuard`.
    struct AsyncCtxGuard;
    impl Drop for AsyncCtxGuard {
        fn drop(&mut self) {
            sema_core::set_async_context(false);
        }
    }

    fn env() -> sema_core::Env {
        let e = sema_core::Env::new();
        register(&e, &Sandbox::allow_all());
        e
    }

    fn call_sync(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let f = env.get_str(name).expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        (nf.func)(&EvalContext::default(), args).expect("sync call ok")
    }

    /// Call a native fn with the async-context gate forced on, then drive
    /// the `AwaitIo` handle it arms to completion by polling. Panics if the
    /// native didn't yield at all (e.g. it silently took the sync fallback)
    /// or the offload rejects.
    fn drive_async(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let f = env.get_str(name).expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        let armed = (nf.func)(&EvalContext::default(), args)
            .expect("native call should arm a yield, not error synchronously");
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
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match handle.poll() {
                sema_core::IoPoll::Ready(Ok(v)) => return v,
                sema_core::IoPoll::Ready(Err(e)) => panic!("offload rejected: {e}"),
                sema_core::IoPoll::Pending => {
                    assert!(
                        Instant::now() < deadline,
                        "offload never completed within 10s"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }

    /// `pty/write` offloads in async context and the write still lands: a
    /// `cat` child under the pty echoes back what was written (the pty's own
    /// terminal echo doubles it, so this just checks the text is present,
    /// same tolerance `pty_runs_a_command`/`isatty_is_true_under_pty` use),
    /// round-tripped entirely through the async path.
    #[test]
    fn write_offloads_and_reaches_child_async() {
        let e = env();
        let h = call_sync(&e, "pty/spawn", &[Value::list(vec![Value::string("cat")])]);

        let result = drive_async(&e, "pty/write", &[h.clone(), Value::string("ping\n")]);
        assert_eq!(result, Value::nil());

        // Ctrl-D (EOF) on its own line ends `cat`'s canonical-mode read,
        // exactly like a user pressing it at a real terminal.
        call_sync(&e, "pty/write", &[h.clone(), Value::string("\u{4}")]);
        let code = call_sync(&e, "pty/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        assert!(call_sync(&e, "pty/read", &[h.clone()])
            .as_str()
            .unwrap()
            .contains("ping"));
        call_sync(&e, "pty/close", &[h]);
    }

    /// `pty/close` offloads `child.kill()` + `child.wait()` in async context
    /// and still frees the registry slot: a follow-up `pty/*` op on the same
    /// handle sees "no such handle", exactly like the sync path leaves it.
    #[test]
    fn close_offloads_kill_and_wait_async() {
        let e = env();
        let h = call_sync(
            &e,
            "pty/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("sleep 30"),
            ])],
        );

        let result = drive_async(&e, "pty/close", &[h.clone()]);
        assert_eq!(result, Value::nil());

        let f = e.get_str("pty/read").expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        let err = (nf.func)(&EvalContext::default(), &[h])
            .expect_err("handle should be freed after close");
        assert!(
            err.to_string().contains("no such handle"),
            "unexpected error: {err}"
        );
    }

    /// `pty/close` in async context on a handle a `pty/wait`/`pty/write`
    /// already has checked out is a hard, immediate `busy` error — never a
    /// queue — exactly matching the sync path (see the module doc comment).
    #[test]
    fn close_on_checked_out_handle_errors_immediately_async() {
        let e = env();
        let h = call_sync(
            &e,
            "pty/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("sleep 30"),
            ])],
        );
        let id = h.as_int().expect("int handle");

        // Simulate an offload in flight by hand: check the real Pty out
        // (keeping it alive locally, not dropped) and leave `CheckedOut`
        // behind, exactly like `poll_wait`'s/`poll_write`'s Acquire phase
        // does mid-offload.
        let checked_out = PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            match ptys.insert(id, PtySlot::CheckedOut) {
                Some(PtySlot::Available(pt)) => pt,
                _ => panic!("expected a freshly spawned Available slot"),
            }
        });

        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let f = e.get_str("pty/close").expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        let err = (nf.func)(&EvalContext::default(), &[h.clone()])
            .expect_err("busy handle should error immediately, not yield");
        assert!(err.to_string().contains("busy"), "unexpected error: {err}");
        sema_core::set_async_context(false);

        // Reinstall the real Pty and clean up through the normal sync path.
        PTYS.with(|p| {
            p.borrow_mut().insert(id, PtySlot::Available(checked_out));
        });
        call_sync(&e, "pty/kill", &[h.clone()]);
        call_sync(&e, "pty/close", &[h]);
    }
}
