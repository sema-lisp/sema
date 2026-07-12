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

    // Optional opts map: {:cwd "path" :env {"KEY" "val" ...}}
    if let Some(opts) = args.get(1) {
        if let Some(m) = opts.as_map_ref() {
            if let Some(cwd) = m.get(&Value::keyword("cwd")).and_then(|v| v.as_str()) {
                cmd.current_dir(cwd);
            }
            if let Some(em) = m.get(&Value::keyword("env")).and_then(|v| v.as_map_ref()) {
                for (k, val) in em.iter() {
                    if let (Some(k), Some(val)) = (k.as_str(), val.as_str()) {
                        cmd.env(k, val);
                    }
                }
            }
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
