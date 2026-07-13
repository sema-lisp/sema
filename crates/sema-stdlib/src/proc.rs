//! Streaming subprocess primitives (`proc/*`).
//!
//! Unlike `shell`, which blocks and returns the full output only after the
//! command exits, these expose a *live* handle: stdout/stderr are drained by
//! background reader threads into buffers you poll with `proc/read-stdout` /
//! `proc/read-stderr`, so a TUI can show test output as it streams. The handle
//! is an integer id into a thread-local registry (the VM is single-threaded, so
//! the registry never crosses threads — only the pipe readers do).
//!
//! `proc/wait` blocks on `Child::wait()`, which can run for the child's whole
//! lifetime. Inside an `async/spawn`'d task that would stall every sibling on
//! the cooperative scheduler, so it offloads through a CHECKOUT: the registry
//! slot (`ProcSlot`) is `Available(Proc)` / `CheckedOut` / `Tombstone(reason)`.
//! `proc/wait` takes the `Proc` (it is `Send` — see the static assertion
//! below) out of the slot for the offload's duration; every other `proc/*` op
//! sees `CheckedOut` and errors clearly rather than racing the background
//! wait for the same `Child`. The offload's poller reinstalls the `Proc` as
//! `Available` and calls `notify_io_complete()` so a sibling task queued on
//! the SAME handle (or any other parked task) can't miss the wakeup. A second
//! `proc/wait` on a handle that's already checked out queues: its `IoHandle`
//! re-attempts the checkout on every poll (the `Acquire` phase) until the slot
//! frees up, then spawns its own offload and switches to `Running` — all
//! under the one `IoHandle` the yield armed. At top level (no scheduler) the
//! sync path is unchanged: it blocks, exactly as before.
//!
//! `proc/write-stdin` and `proc/close` reuse the same `ProcSlot` CHECKOUT.
//! `proc/write-stdin` mirrors `proc/wait`'s `Acquire`/`Running` shape
//! exactly (see `WriteStdinPhase`/`poll_write_stdin`), offloading
//! `sin.write_all(text) + flush()` — a large write to a child that isn't
//! draining its stdin blocks in-kernel on the pipe buffer. `proc/close`
//! checks its handle out synchronously (`kill()` is a signal send, not a
//! wait, so that step stays on the VM thread — and a busy handle is a hard,
//! immediate error, never a queue, matching the sync path), then reuses
//! `spawn_proc_wait` itself to offload `child.wait()` + the pump-thread
//! joins, discarding the reaped `Proc` on completion instead of
//! reinstalling it (`proc/close` frees the slot).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use sema_core::{check_arity, in_async_context, Caps, IoHandle, IoPoll, SemaError, Value};

struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
    out_thread: Option<JoinHandle<()>>,
    err_thread: Option<JoinHandle<()>>,
}

// `proc/wait`'s offload moves a whole `Proc` onto the I/O pool's blocking
// tier and back. This compiles only if every field stays `Send`; a future
// field addition that breaks it fails here, not with an opaque trait-bound
// error deep in `sema_io::io_spawn_blocking`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Proc>();
};

/// A registry slot. `CheckedOut` is the moment between `proc/wait` taking the
/// `Proc` out for its offload and the poller reinstalling it; every other
/// `proc/*` op treats it as "busy, try again once the wait resolves".
/// `Tombstone` is terminal: set only when a `proc/wait` offload is cancelled
/// mid-flight (the `Proc` is stuck inside an uncancellable background thread —
/// see `spawn_proc_wait`'s doc comment) or its worker vanishes unexpectedly;
/// `proc/close` is the only way to free a tombstoned slot.
enum ProcSlot {
    Available(Proc),
    CheckedOut,
    Tombstone(String),
}

thread_local! {
    static PROCS: RefCell<HashMap<i64, ProcSlot>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<i64> = const { Cell::new(1) };
}

/// Spawn a thread that drains `reader` into `buf` until EOF. The returned
/// handle is joined by `proc/wait`: a finished join means EOF was reached, so
/// every byte the child wrote is in `buf` (the tail-buffering guarantee).
fn pump<R: Read + Send + 'static>(mut reader: R, buf: Arc<Mutex<Vec<u8>>>) -> JoinHandle<()> {
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

/// Take the integer handle from `args[idx]`.
fn handle(args: &[Value], idx: usize) -> Result<i64, SemaError> {
    args[idx]
        .as_int()
        .ok_or_else(|| SemaError::type_error("integer (proc handle)", args[idx].type_name()))
}

/// Drain a buffer's current contents as a lossy-UTF-8 string (clearing it).
fn drain(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
    let s = String::from_utf8_lossy(&b).into_owned();
    b.clear();
    s
}

fn missing_err(op: &str, id: i64) -> SemaError {
    SemaError::eval(format!("{op}: no such handle {id}"))
}

/// `op` was attempted while `proc/wait` had this handle checked out (its
/// blocking wait is running on the I/O pool).
fn busy_err(op: &str, id: i64) -> SemaError {
    SemaError::eval(format!(
        "{op}: handle {id} is busy — a proc/wait is in flight on it"
    ))
    .with_hint("wait for the in-flight proc/wait to resolve before calling another proc/* op on this handle")
}

/// `op` was attempted on a handle whose in-flight `proc/wait` was cancelled.
fn tombstone_err(op: &str, id: i64, reason: &str) -> SemaError {
    SemaError::eval(format!("{op}: handle {id} is no longer usable: {reason}"))
}

/// Look up `id` for an op that needs `&mut Proc`, translating the other slot
/// states into a clear, `op`-specific error instead of ever panicking on the
/// enum shape.
fn with_proc<R>(
    op: &str,
    id: i64,
    f: impl FnOnce(&mut Proc) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    PROCS.with(|p| {
        let mut procs = p.borrow_mut();
        match procs.get_mut(&id) {
            Some(ProcSlot::Available(pr)) => f(pr),
            Some(ProcSlot::CheckedOut) => Err(busy_err(op, id)),
            Some(ProcSlot::Tombstone(msg)) => Err(tombstone_err(op, id, msg)),
            None => Err(missing_err(op, id)),
        }
    })
}

/// Poll a process handle for `event/select`: `Some((has_buffered_output,
/// has_exited))`, or `None` if the handle is unknown OR currently checked out
/// by an in-flight `proc/wait` (treated the same as "not ready yet" — this is
/// a best-effort poll, never an error). Drives the TUI's "show test output as
/// it streams, then react to exit" loop.
pub(crate) fn poll_ready(id: i64) -> Option<(bool, bool)> {
    PROCS.with(|p| {
        let mut procs = p.borrow_mut();
        match procs.get_mut(&id) {
            Some(ProcSlot::Available(pr)) => {
                let has_out = pr.out.lock().map(|b| !b.is_empty()).unwrap_or(false)
                    || pr.err.lock().map(|b| !b.is_empty()).unwrap_or(false);
                let exited = matches!(pr.child.try_wait(), Ok(Some(_)));
                Some((has_out, exited))
            }
            _ => None,
        }
    })
}

fn spawn(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "proc/spawn", 1..=2);
    let argv = args[0]
        .as_list()
        .or_else(|| args[0].as_vector())
        .ok_or_else(|| SemaError::type_error("list of strings (argv)", args[0].type_name()))?;
    if argv.is_empty() {
        return Err(SemaError::eval("proc/spawn: argv must be non-empty"));
    }
    let mut parts: Vec<String> = Vec::with_capacity(argv.len());
    for v in argv {
        parts.push(
            v.as_str()
                .ok_or_else(|| SemaError::type_error("string", v.type_name()))?
                .to_string(),
        );
    }

    let mut cmd = Command::new(&parts[0]);
    cmd.args(&parts[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Optional opts map: {:cwd "path" :env {"KEY" "val" ...}}. Shared extraction
    // with `shell` so both APIs interpret the map identically.
    if let Some(m) = args.get(1).and_then(|v| v.as_map_ref()) {
        let (cwd, env_vars) = crate::system::command_opts(m);
        if let Some(dir) = &cwd {
            cmd.current_dir(dir);
        }
        for (k, val) in &env_vars {
            cmd.env(k, val);
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| SemaError::eval(format!("proc/spawn {}: {e}", parts[0])))?;

    let out = Arc::new(Mutex::new(Vec::new()));
    let err = Arc::new(Mutex::new(Vec::new()));
    let out_thread = child.stdout.take().map(|so| pump(so, out.clone()));
    let err_thread = child.stderr.take().map(|se| pump(se, err.clone()));
    let stdin = child.stdin.take();

    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    PROCS.with(|p| {
        p.borrow_mut().insert(
            id,
            ProcSlot::Available(Proc {
                child,
                stdin,
                out,
                err,
                out_thread,
                err_thread,
            }),
        )
    });
    Ok(Value::int(id))
}

/// What crosses the thread boundary from the offloaded `child.wait()` back to
/// the poller: the reaped `Proc` (its pump-thread `JoinHandle`s consumed —
/// joining them guarantees every byte the child wrote is buffered, exactly
/// like the sync path) plus the wait outcome. Only `Send` data ever crosses —
/// never a `Value`/`Rc`.
struct WaitOutcome {
    proc: Proc,
    status: Result<i32, String>,
}

/// Move `proc`'s blocking `child.wait()` — plus joining the stdout/stderr pump
/// threads — onto the I/O pool's blocking tier. Cancellation past this point
/// is best-effort by construction (the `Proc` is inside a `spawn_blocking`
/// closure with no abort hook, the same tradeoff every other spawn_blocking-
/// based offload in this codebase accepts — see `IoHandle::with_abort`'s doc
/// comment): the caller marks the registry slot `Tombstone` on abort so a
/// later access errors clearly instead of the slot staying `CheckedOut`
/// forever with no one left to reinstall it, but the OS process itself keeps
/// running (or finishes) unattended.
fn spawn_proc_wait(mut proc: Proc) -> tokio::sync::oneshot::Receiver<WaitOutcome> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let status = proc
            .child
            .wait()
            .map(|s| s.code().unwrap_or(-1))
            .map_err(|e| format!("proc/wait: {e}"));
        if let Some(t) = proc.out_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = proc.err_thread.take() {
            let _ = t.join();
        }
        let _ = tx.send(WaitOutcome { proc, status });
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });
    rx
}

/// The two phases a `proc/wait` `IoHandle` cycles through. A caller that finds
/// the slot immediately `Available` still starts in `Acquire` — it succeeds on
/// the very first poll and falls through into `Running` in the same tick, so
/// there is exactly one code path for both the uncontended and the queued
/// case (see `poll_wait`).
enum WaitPhase {
    /// Waiting for the slot to become `Available`. Re-checked every poll;
    /// never mutates anything beyond that check, so aborting here is a true
    /// no-op — nothing was ever taken out.
    Acquire,
    /// Holding the checkout; the blocking wait+join is running on the I/O
    /// pool. Resolves with the reinstalled `Proc` plus the wait outcome.
    Running(tokio::sync::oneshot::Receiver<WaitOutcome>),
}

/// Poll (and drive) one `proc/wait`'s `Acquire` → `Running` state machine.
fn poll_wait(id: i64, phase: &mut WaitPhase) -> IoPoll {
    use tokio::sync::oneshot::error::TryRecvError;
    loop {
        match phase {
            WaitPhase::Acquire => {
                enum Acquired {
                    Not,
                    Proc(Proc),
                    Err(String),
                }
                let acquired = PROCS.with(|p| {
                    let mut procs = p.borrow_mut();
                    match procs.get_mut(&id) {
                        Some(slot @ ProcSlot::Available(_)) => {
                            let ProcSlot::Available(pr) =
                                std::mem::replace(slot, ProcSlot::CheckedOut)
                            else {
                                unreachable!("just matched Available")
                            };
                            Acquired::Proc(pr)
                        }
                        Some(ProcSlot::CheckedOut) => Acquired::Not,
                        Some(ProcSlot::Tombstone(msg)) => {
                            Acquired::Err(tombstone_err("proc/wait", id, msg).to_string())
                        }
                        None => Acquired::Err(missing_err("proc/wait", id).to_string()),
                    }
                });
                match acquired {
                    Acquired::Not => return IoPoll::Pending,
                    Acquired::Err(msg) => return IoPoll::Ready(Err(msg)),
                    Acquired::Proc(pr) => {
                        *phase = WaitPhase::Running(spawn_proc_wait(pr));
                        // Fall through: poll the freshly spawned receiver
                        // immediately instead of wasting a scheduler tick.
                    }
                }
            }
            WaitPhase::Running(rx) => {
                return match rx.try_recv() {
                    Err(TryRecvError::Empty) => IoPoll::Pending,
                    Ok(outcome) => {
                        PROCS
                            .with(|p| p.borrow_mut().insert(id, ProcSlot::Available(outcome.proc)));
                        // MANDATORY lost-wakeup guard: a sibling queued on this
                        // same handle (still in `Acquire`) may have polled
                        // Pending earlier in this scheduler sweep — without
                        // this it would park until an unrelated wakeup.
                        sema_core::notify_io_complete();
                        match outcome.status {
                            Ok(code) => IoPoll::Ready(Ok(Value::int(code as i64))),
                            Err(msg) => IoPoll::Ready(Err(SemaError::Io(msg).to_string())),
                        }
                    }
                    Err(TryRecvError::Closed) => {
                        PROCS.with(|p| {
                            p.borrow_mut().insert(
                                id,
                                ProcSlot::Tombstone(
                                    "the wait worker terminated unexpectedly".to_string(),
                                ),
                            )
                        });
                        IoPoll::Ready(Err("proc/wait: subprocess wait worker dropped".to_string()))
                    }
                };
            }
        }
    }
}

/// The async-context `proc/wait` entry point: yields `AwaitIo` and lets the
/// scheduler drive `poll_wait` to completion instead of blocking the VM
/// thread on `Child::wait()`.
fn proc_wait_async(id: i64) -> Result<Value, SemaError> {
    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let phase = Rc::new(RefCell::new(WaitPhase::Acquire));
    let phase_for_poll = phase.clone();
    let handle = Rc::new(IoHandle::with_abort(
        move || poll_wait(id, &mut phase_for_poll.borrow_mut()),
        move || {
            // Acquire-phase abort: no-op — nothing was ever checked out, the
            // registry slot is exactly as another caller left it. Running-
            // phase abort: best-effort — see `spawn_proc_wait`'s doc comment.
            if matches!(*phase.borrow(), WaitPhase::Running(_)) {
                PROCS.with(|p| {
                    p.borrow_mut().insert(
                        id,
                        ProcSlot::Tombstone(
                            "proc/wait was cancelled while the wait was in flight; the \
                             process may still be running in the background but this \
                             handle can no longer reach it — proc/close frees the slot; \
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

/// What crosses the thread boundary from the offloaded `sin.write_all` +
/// `flush` back to the poller: the reinstalled `Proc` plus the write
/// outcome. Only `Send` data ever crosses — the bytes to write are copied
/// into an owned `Vec<u8>` before the offload starts, never a `Value`.
struct WriteOutcome {
    proc: Proc,
    result: Result<(), String>,
}

/// Move `proc`'s blocking `sin.write_all(text) + flush()` onto the I/O
/// pool's blocking tier — a large write to a child that isn't draining its
/// stdin blocks in-kernel on the pipe buffer, same tradeoff as
/// `spawn_proc_wait`. `text` is a plain owned `Vec<u8>` so the closure stays
/// `Send + 'static`.
fn spawn_proc_write_stdin(
    mut proc: Proc,
    text: Vec<u8>,
) -> tokio::sync::oneshot::Receiver<WriteOutcome> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let result = match proc.stdin.as_mut() {
            Some(sin) => sin
                .write_all(&text)
                .and_then(|_| sin.flush())
                .map_err(|e| format!("proc/write-stdin: {e}")),
            None => Err("proc/write-stdin: stdin already closed".to_string()),
        };
        let _ = tx.send(WriteOutcome { proc, result });
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });
    rx
}

/// The two phases a `proc/write-stdin` `IoHandle` cycles through — same
/// Acquire/Running shape as `WaitPhase` (see `poll_wait`), except `Acquire`
/// also carries the pending bytes so they can be moved into the offload
/// exactly once, on the transition into `Running`.
enum WriteStdinPhase {
    Acquire(Vec<u8>),
    Running(tokio::sync::oneshot::Receiver<WriteOutcome>),
}

/// Poll (and drive) one `proc/write-stdin`'s `Acquire` → `Running` state
/// machine. Mirrors `poll_wait` field-for-field.
fn poll_write_stdin(id: i64, phase: &mut WriteStdinPhase) -> IoPoll {
    use tokio::sync::oneshot::error::TryRecvError;
    loop {
        match phase {
            WriteStdinPhase::Acquire(_) => {
                enum Acquired {
                    Not,
                    Proc(Proc),
                    Err(String),
                }
                let acquired = PROCS.with(|p| {
                    let mut procs = p.borrow_mut();
                    match procs.get_mut(&id) {
                        Some(slot @ ProcSlot::Available(_)) => {
                            let ProcSlot::Available(pr) =
                                std::mem::replace(slot, ProcSlot::CheckedOut)
                            else {
                                unreachable!("just matched Available")
                            };
                            Acquired::Proc(pr)
                        }
                        Some(ProcSlot::CheckedOut) => Acquired::Not,
                        Some(ProcSlot::Tombstone(msg)) => {
                            Acquired::Err(tombstone_err("proc/write-stdin", id, msg).to_string())
                        }
                        None => Acquired::Err(missing_err("proc/write-stdin", id).to_string()),
                    }
                });
                match acquired {
                    Acquired::Not => return IoPoll::Pending,
                    Acquired::Err(msg) => return IoPoll::Ready(Err(msg)),
                    Acquired::Proc(pr) => {
                        let WriteStdinPhase::Acquire(text) = phase else {
                            unreachable!("just matched Acquire")
                        };
                        let text = std::mem::take(text);
                        *phase = WriteStdinPhase::Running(spawn_proc_write_stdin(pr, text));
                        // Fall through: poll the freshly spawned receiver
                        // immediately instead of wasting a scheduler tick.
                    }
                }
            }
            WriteStdinPhase::Running(rx) => {
                return match rx.try_recv() {
                    Err(TryRecvError::Empty) => IoPoll::Pending,
                    Ok(outcome) => {
                        PROCS
                            .with(|p| p.borrow_mut().insert(id, ProcSlot::Available(outcome.proc)));
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
                        PROCS.with(|p| {
                            p.borrow_mut().insert(
                                id,
                                ProcSlot::Tombstone(
                                    "the write-stdin worker terminated unexpectedly".to_string(),
                                ),
                            )
                        });
                        IoPoll::Ready(Err(
                            "proc/write-stdin: subprocess write worker dropped".to_string()
                        ))
                    }
                };
            }
        }
    }
}

/// The async-context `proc/write-stdin` entry point: yields `AwaitIo` and
/// lets the scheduler drive `poll_write_stdin` to completion instead of
/// blocking the VM thread on `sin.write_all`.
fn proc_write_stdin_async(id: i64, text: Vec<u8>) -> Result<Value, SemaError> {
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let phase = Rc::new(RefCell::new(WriteStdinPhase::Acquire(text)));
    let phase_for_poll = phase.clone();
    let handle = Rc::new(IoHandle::with_abort(
        move || poll_write_stdin(id, &mut phase_for_poll.borrow_mut()),
        move || {
            // Acquire-phase abort: no-op, nothing was checked out yet.
            // Running-phase abort: best-effort, same tradeoff as
            // `proc/wait`'s abort (see `spawn_proc_wait`'s doc comment) — the
            // write may still be landing on the worker with no way to
            // interrupt it.
            if matches!(*phase.borrow(), WriteStdinPhase::Running(_)) {
                PROCS.with(|p| {
                    p.borrow_mut().insert(
                        id,
                        ProcSlot::Tombstone(
                            "proc/write-stdin was cancelled while the write was in flight; \
                             the write may have partially landed but this handle can no \
                             longer reach it — proc/close frees the slot"
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

/// The state `proc/close`'s async offload passes through while its
/// `spawn_proc_wait` offload (started right after the synchronous, non-
/// blocking `kill()`) runs on the I/O pool. Unlike `proc/wait`'s
/// `WaitPhase`, there is no `Acquire` retry here: `proc/close` never queues
/// behind a busy handle — a handle a `proc/wait` already has checked out is
/// a hard, immediate `busy_err`, exactly like the sync path.
struct ClosePhase(tokio::sync::oneshot::Receiver<WaitOutcome>);

/// Poll `proc/close`'s in-flight `spawn_proc_wait` offload to completion.
fn poll_close(id: i64, phase: &mut ClosePhase) -> IoPoll {
    use tokio::sync::oneshot::error::TryRecvError;
    match phase.0.try_recv() {
        Err(TryRecvError::Empty) => IoPoll::Pending,
        Ok(_outcome) => {
            // Unlike `proc/wait`, `proc/close` frees the slot instead of
            // reinstalling it — the reaped `Proc` (pump threads already
            // joined inside the offload) is simply dropped here, exactly
            // like the sync path drops it after `procs.remove(&id)`.
            PROCS.with(|p| {
                p.borrow_mut().remove(&id);
            });
            sema_core::notify_io_complete();
            IoPoll::Ready(Ok(Value::nil()))
        }
        Err(TryRecvError::Closed) => {
            PROCS.with(|p| {
                p.borrow_mut().insert(
                    id,
                    ProcSlot::Tombstone("the close worker terminated unexpectedly".to_string()),
                )
            });
            IoPoll::Ready(Err("proc/close: subprocess wait worker dropped".to_string()))
        }
    }
}

/// The async-context `proc/close` entry point. Mirrors the sync path exactly
/// up through `kill()` — checked out synchronously on the VM thread (`kill`
/// is a signal send, not a wait, so this stays cheap), `busy_err` on a
/// handle a `proc/wait` already has checked out, silent no-op on a
/// `Tombstone`/missing handle — then offloads only the blocking
/// `child.wait()` + pump-thread joins, reusing `spawn_proc_wait` exactly as
/// `proc/wait` does instead of duplicating it.
fn proc_close_async(id: i64) -> Result<Value, SemaError> {
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let taken: Option<Proc> = PROCS.with(|p| -> Result<Option<Proc>, SemaError> {
        let mut procs = p.borrow_mut();
        if matches!(procs.get(&id), Some(ProcSlot::CheckedOut)) {
            return Err(busy_err("proc/close", id));
        }
        Ok(match procs.remove(&id) {
            Some(ProcSlot::Available(pr)) => Some(pr),
            // Tombstone or missing: already unusable/gone — the sync path
            // treats both as a silent no-op via the same unconditional
            // `procs.remove(&id)`, so does this one.
            _ => None,
        })
    })?;

    let Some(mut pr) = taken else {
        return Ok(Value::nil());
    };

    let _ = pr.child.kill(); // a signal send — cheap and non-blocking, not a wait

    // Mark the slot busy for the offloaded wait+join's duration so a
    // concurrent proc/* op on this id sees a clear "busy" error instead of
    // racing the pump-thread join or seeing "missing handle" mid-reap.
    PROCS.with(|p| {
        p.borrow_mut().insert(id, ProcSlot::CheckedOut);
    });

    let phase = Rc::new(RefCell::new(ClosePhase(spawn_proc_wait(pr))));
    let phase_for_poll = phase.clone();
    let handle = Rc::new(IoHandle::with_abort(
        move || poll_close(id, &mut phase_for_poll.borrow_mut()),
        move || {
            // Best-effort, same tradeoff as `proc/wait`'s abort (see
            // `spawn_proc_wait`'s doc comment): the child is already killed,
            // but its wait+join keeps running unattended inside
            // `spawn_blocking` with no abort hook, so the slot is
            // tombstoned rather than left `CheckedOut` forever.
            PROCS.with(|p| {
                p.borrow_mut().insert(
                    id,
                    ProcSlot::Tombstone(
                        "proc/close was cancelled while reaping the killed process; the \
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
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/spawn", spawn);

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/read-stdout", |args| {
        check_arity!(args, "proc/read-stdout", 1);
        let id = handle(args, 0)?;
        with_proc("proc/read-stdout", id, |pr| {
            Ok(Value::string(&drain(&pr.out)))
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/read-stderr", |args| {
        check_arity!(args, "proc/read-stderr", 1);
        let id = handle(args, 0)?;
        with_proc("proc/read-stderr", id, |pr| {
            Ok(Value::string(&drain(&pr.err)))
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/write-stdin", |args| {
        check_arity!(args, "proc/write-stdin", 2);
        let id = handle(args, 0)?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        if in_async_context() {
            return proc_write_stdin_async(id, text.as_bytes().to_vec());
        }
        with_proc("proc/write-stdin", id, |pr| match pr.stdin.as_mut() {
            Some(sin) => {
                sin.write_all(text.as_bytes())
                    .and_then(|_| sin.flush())
                    .map_err(|e| SemaError::Io(format!("proc/write-stdin: {e}")))?;
                Ok(Value::nil())
            }
            None => Err(SemaError::eval("proc/write-stdin: stdin already closed")),
        })
    });

    // proc/close-stdin — send EOF to the child by dropping its stdin.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/close-stdin", |args| {
        check_arity!(args, "proc/close-stdin", 1);
        let id = handle(args, 0)?;
        with_proc("proc/close-stdin", id, |pr| {
            pr.stdin = None; // drop → EOF
            Ok(Value::nil())
        })
    });

    // proc/wait — block until exit, return the exit code (or -1 if signalled).
    // Async context offloads via `proc_wait_async` (see the module doc
    // comment for the checkout design); top level keeps the original
    // synchronous shape byte-for-byte: the handle is removed from the
    // registry first so the blocking wait doesn't hold the thread-local
    // borrow, then joining the pump threads guarantees every byte the child
    // wrote is buffered before we return (so a following proc/read-stdout
    // sees the tail), then the proc is re-inserted so reads still work after
    // wait — including a second proc/wait, since `Child::wait` caches and
    // returns the same status once the child is reaped (verified: this is
    // the behavior being preserved, not introduced).
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/wait", |args| {
        check_arity!(args, "proc/wait", 1);
        let id = handle(args, 0)?;
        if in_async_context() {
            return proc_wait_async(id);
        }
        let mut pr = PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            match procs.remove(&id) {
                Some(ProcSlot::Available(pr)) => Ok(pr),
                // A real (if narrow) interleaving, not just defensive
                // paranoia: top-level code can `await` something else,
                // driving the scheduler far enough for a concurrently
                // spawned task's proc/wait to check this same handle out,
                // then call proc/wait itself before that task resumes.
                Some(slot @ ProcSlot::CheckedOut) => {
                    procs.insert(id, slot);
                    Err(busy_err("proc/wait", id))
                }
                Some(ProcSlot::Tombstone(msg)) => Err(tombstone_err("proc/wait", id, &msg)),
                None => Err(missing_err("proc/wait", id)),
            }
        })?;
        let status = pr.child.wait();
        if let Some(t) = pr.out_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = pr.err_thread.take() {
            let _ = t.join();
        }
        PROCS.with(|p| p.borrow_mut().insert(id, ProcSlot::Available(pr)));
        let status = status.map_err(|e| SemaError::Io(format!("proc/wait: {e}")))?;
        Ok(Value::int(status.code().unwrap_or(-1) as i64))
    });

    // proc/exit-code — Some(code) if exited, nil if still running.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/exit-code", |args| {
        check_arity!(args, "proc/exit-code", 1);
        let id = handle(args, 0)?;
        with_proc("proc/exit-code", id, |pr| {
            match pr
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("proc/exit-code: {e}")))?
            {
                Some(status) => Ok(Value::int(status.code().unwrap_or(-1) as i64)),
                None => Ok(Value::nil()),
            }
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/running?", |args| {
        check_arity!(args, "proc/running?", 1);
        let id = handle(args, 0)?;
        with_proc("proc/running?", id, |pr| {
            let running = pr
                .child
                .try_wait()
                .map_err(|e| SemaError::Io(format!("proc/running?: {e}")))?
                .is_none();
            Ok(Value::bool(running))
        })
    });

    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/kill", |args| {
        check_arity!(args, "proc/kill", 1);
        let id = handle(args, 0)?;
        with_proc("proc/kill", id, |pr| {
            let _ = pr.child.kill(); // ignore "already exited"
            Ok(Value::nil())
        })
    });

    // proc/close — kill if needed and drop the handle (frees the registry
    // slot). Missing/already-closed and tombstoned handles are a silent
    // no-op (today's missing-handle behavior, extended to tombstones so
    // proc/close remains the documented way to free one). A handle that is
    // busy (a proc/wait offload holds it) errors instead of racing the
    // background wait for the same Child — wait for it (or let it finish in
    // the background) before closing.
    crate::register_fn_gated(env, sandbox, Caps::PROCESS, "proc/close", |args| {
        check_arity!(args, "proc/close", 1);
        let id = handle(args, 0)?;
        if in_async_context() {
            return proc_close_async(id);
        }
        PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            if matches!(procs.get(&id), Some(ProcSlot::CheckedOut)) {
                return Err(busy_err("proc/close", id));
            }
            if let Some(ProcSlot::Available(mut pr)) = procs.remove(&id) {
                let _ = pr.child.kill();
                let _ = pr.child.wait();
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
    fn spawn_read_wait_roundtrip() {
        let e = env();
        let h = call(
            &e,
            "proc/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("printf hello; printf oops 1>&2"),
            ])],
        );
        let code = call(&e, "proc/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        assert_eq!(
            call(&e, "proc/read-stdout", &[h.clone()]).as_str(),
            Some("hello")
        );
        assert_eq!(
            call(&e, "proc/read-stderr", &[h.clone()]).as_str(),
            Some("oops")
        );
        call(&e, "proc/close", &[h]);
    }

    #[test]
    fn write_stdin_echoes() {
        let e = env();
        let h = call(&e, "proc/spawn", &[Value::list(vec![Value::string("cat")])]);
        call(
            &e,
            "proc/write-stdin",
            &[h.clone(), Value::string("ping\n")],
        );
        call(&e, "proc/close-stdin", &[h.clone()]);
        let code = call(&e, "proc/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        assert_eq!(
            call(&e, "proc/read-stdout", &[h.clone()]).as_str(),
            Some("ping\n")
        );
        call(&e, "proc/close", &[h]);
    }

    /// Today's behavior (sync path): `proc/wait` may be called more than once
    /// on the same handle and returns the identical exit code each time —
    /// `Child::wait` caches the status once the child is reaped rather than
    /// erroring on a repeat call. The async path (proc_pty_async_test.rs)
    /// asserts this stays true through the offload.
    #[test]
    fn double_wait_returns_same_code_sync() {
        let e = env();
        let h = call(
            &e,
            "proc/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("exit 7"),
            ])],
        );
        let first = call(&e, "proc/wait", &[h.clone()]);
        let second = call(&e, "proc/wait", &[h.clone()]);
        assert_eq!(first.as_int(), Some(7));
        assert_eq!(second.as_int(), Some(7));
        call(&e, "proc/close", &[h]);
    }
}

/// Async-context coverage for the `proc/write-stdin` / `proc/close`
/// scheduler offloads added to this file. `sema-stdlib` doesn't depend on
/// `sema-vm`/`sema-eval` (the real scheduler + interpreter live there), so
/// these tests stand in for the scheduler by hand: force
/// `sema_core::in_async_context()` on, call the native, then poll the
/// `AwaitIo` handle it arms to completion — exactly what the scheduler does
/// in production, just single-threaded and synchronous here. Mirrors
/// `io.rs`'s `async_offload_tests` module.
#[cfg(test)]
mod async_offload_tests {
    use super::*;
    use sema_core::{EvalContext, Sandbox};
    use std::time::{Duration, Instant};

    /// Forces `in_async_context()` on for the guard's lifetime, resetting it
    /// (even on panic/early return) so a failure can't leak the flag into
    /// whichever test the harness runs next on the same worker thread —
    /// mirrors `io.rs`'s `AsyncCtxGuard`.
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

    /// `proc/write-stdin` offloads in async context and the write still
    /// lands: a `cat` child echoes back exactly what was written, round-
    /// tripped entirely through the async path (write, then — back at top
    /// level — close-stdin/wait/read to observe the result).
    #[test]
    fn write_stdin_offloads_and_echoes_async() {
        let e = env();
        let h = call_sync(&e, "proc/spawn", &[Value::list(vec![Value::string("cat")])]);

        let result = drive_async(
            &e,
            "proc/write-stdin",
            &[h.clone(), Value::string("ping\n")],
        );
        assert_eq!(result, Value::nil());

        // Back at top level: close stdin to send EOF, wait, then read —
        // proving the offloaded write actually reached the child.
        call_sync(&e, "proc/close-stdin", &[h.clone()]);
        let code = call_sync(&e, "proc/wait", &[h.clone()]);
        assert_eq!(code.as_int(), Some(0));
        assert_eq!(
            call_sync(&e, "proc/read-stdout", &[h.clone()]).as_str(),
            Some("ping\n")
        );
        call_sync(&e, "proc/close", &[h]);
    }

    /// `proc/write-stdin` in async context on a handle with stdin already
    /// closed rejects with the same message the sync path uses, just
    /// delivered through the offload's `Ready(Err(..))` instead of a
    /// direct `Result::Err`.
    #[test]
    fn write_stdin_after_close_stdin_errors_async() {
        let e = env();
        let h = call_sync(&e, "proc/spawn", &[Value::list(vec![Value::string("cat")])]);
        call_sync(&e, "proc/close-stdin", &[h.clone()]);

        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let f = e.get_str("proc/write-stdin").expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        let armed = (nf.func)(&EvalContext::default(), &[h.clone(), Value::string("nope")])
            .expect("arms a yield");
        assert_eq!(armed, Value::nil());
        let reason = sema_core::take_yield_signal().expect("yield armed");
        let handle = match reason {
            sema_core::YieldReason::AwaitIo(h) => h,
            other => panic!("expected AwaitIo, got {other:?}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        let err = loop {
            match handle.poll() {
                sema_core::IoPoll::Ready(Err(e)) => break e,
                sema_core::IoPoll::Ready(Ok(v)) => panic!("expected an error, got {v:?}"),
                sema_core::IoPoll::Pending => {
                    assert!(Instant::now() < deadline, "never completed within 10s");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        };
        assert!(
            err.contains("stdin already closed"),
            "unexpected error: {err}"
        );
        sema_core::set_async_context(false);
        call_sync(&e, "proc/kill", &[h.clone()]);
        call_sync(&e, "proc/close", &[h]);
    }

    /// `proc/close` offloads `child.kill()` + `child.wait()` in async
    /// context and still frees the registry slot: a follow-up `proc/*` op
    /// on the same handle sees "no such handle", exactly like the sync
    /// path leaves it.
    #[test]
    fn close_offloads_kill_and_wait_async() {
        let e = env();
        let h = call_sync(
            &e,
            "proc/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("sleep 30"),
            ])],
        );

        let result = drive_async(&e, "proc/close", &[h.clone()]);
        assert_eq!(result, Value::nil());

        // The slot is freed — a following sync op errors "no such handle",
        // same as the sync `proc/close` path leaves it.
        let f = e.get_str("proc/read-stdout").expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        let err = (nf.func)(&EvalContext::default(), &[h])
            .expect_err("handle should be freed after close");
        assert!(
            err.to_string().contains("no such handle"),
            "unexpected error: {err}"
        );
    }

    /// `proc/close` in async context on a handle a `proc/wait` already has
    /// checked out is a hard, immediate `busy` error — never a queue —
    /// exactly matching the sync path (see the module doc comment).
    #[test]
    fn close_on_checked_out_handle_errors_immediately_async() {
        let e = env();
        let h = call_sync(
            &e,
            "proc/spawn",
            &[Value::list(vec![
                Value::string("sh"),
                Value::string("-c"),
                Value::string("sleep 30"),
            ])],
        );
        let id = h.as_int().expect("int handle");

        // Simulate a proc/wait offload in flight by hand: check the real
        // Proc out (keeping it alive locally, not dropped) and leave
        // `CheckedOut` behind, exactly like `poll_wait`'s Acquire phase
        // does mid-offload.
        let checked_out = PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            match procs.insert(id, ProcSlot::CheckedOut) {
                Some(ProcSlot::Available(pr)) => pr,
                _ => panic!("expected a freshly spawned Available slot"),
            }
        });

        let _guard = AsyncCtxGuard;
        sema_core::set_async_context(true);
        let f = e.get_str("proc/close").expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        let err = (nf.func)(&EvalContext::default(), &[h.clone()])
            .expect_err("busy handle should error immediately, not yield");
        assert!(err.to_string().contains("busy"), "unexpected error: {err}");
        sema_core::set_async_context(false);

        // Reinstall the real Proc and clean up through the normal sync path.
        PROCS.with(|p| {
            p.borrow_mut().insert(id, ProcSlot::Available(checked_out));
        });
        call_sync(&e, "proc/close", &[h]);
    }
}
