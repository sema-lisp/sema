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
//! the cooperative scheduler, so it offloads through the CHECKOUT pattern under
//! the unified runtime via [`crate::runtime_offload::checkout_external`] (see
//! `sqlite.rs`'s module doc comment for the canonical writeup this mirrors): the
//! registry slot (`ProcSlot`) is `Available(Proc)` / `CheckedOut` /
//! `Tombstone(reason)`, guarded by a per-handle `ResourceGate` that serializes
//! concurrent ops FIFO. `proc/wait` acquires the gate, takes the `Proc` (it is
//! `Send` — see the static assertion below) out of the slot, runs `child.wait()`
//! plus the pump-thread joins on the executor's blocking tier, then reinstalls
//! the `Proc` and releases the gate. A second `proc/wait` on a busy handle parks
//! FIFO on the gate; every non-offloaded `proc/*` op sees `CheckedOut` and errors
//! clearly rather than racing the background wait. A mid-flight cancel tombstones
//! the slot (best-effort — the blocking wait keeps running unattended) and
//! SIGKILLs the child's process group (which also unsticks the worker). At top
//! level (no scheduler) the sync path is unchanged: it blocks, exactly as before.
//!
//! `proc/write-stdin` and `proc/close` reuse the same `ProcSlot` CHECKOUT.
//! `proc/write-stdin` offloads `sin.write_all(text) + flush()` — a large write
//! to a child that isn't draining its stdin blocks in-kernel on the pipe buffer.
//! `proc/close` kills its handle synchronously (`kill()` is a signal send, not a
//! wait, so that step stays on the VM thread — and a busy handle is a hard,
//! immediate error, never a queue, matching the sync path), then offloads
//! `child.wait()` + the pump-thread joins through the same checkout, discarding
//! the reaped `Proc` and freeing the slot + gate on completion instead of
//! reinstalling it (`proc/close` frees the handle).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult, ResourceGateHandle};
use sema_core::{check_arity, in_runtime_quantum, Caps, SemaError, Value};

use crate::runtime_offload::{
    checkout_external, finish_terminal_gate, group_sigkill_abort, prepare_terminal_gate,
    suspend_terminal_external, CheckoutOp,
};

/// Completion-kind tag for `proc/*` external waits ("proc").
const PROC_COMPLETION_KIND: u64 = 0x7072_6f63;

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

/// A registry slot. `CheckedOut` is the moment between a checkout taking the
/// `Proc` out for its offload and the decoder reinstalling it; every other
/// `proc/*` op treats it as "busy, try again once the wait resolves".
/// `Tombstone` is terminal: set only when a checkout op is cancelled mid-flight
/// (the `Proc` is stuck inside an uncancellable blocking worker — best-effort
/// cancellation) or its worker vanishes unexpectedly; `proc/close` is the only
/// way to free a tombstoned slot.
enum ProcSlot {
    Available(Proc),
    CheckedOut,
    Tombstone(String),
}

thread_local! {
    static PROCS: RefCell<HashMap<i64, ProcSlot>> = RefCell::new(HashMap::new());
    static NEXT_ID: Cell<i64> = const { Cell::new(1) };
    /// Per-handle owning resource-gate capability, created lazily on the first
    /// offloaded op and reused for later ops (dropped on `proc/close`). The gate
    /// provides FIFO mutual exclusion for the checkout slot.
    static PROC_GATES: RefCell<HashMap<i64, ResourceGateHandle>> = RefCell::new(HashMap::new());
}

/// Take `id`'s process out of its slot once its gate is owned, marking the slot
/// `CheckedOut`. A tombstoned/missing/busy slot fails with the same clear text
/// the sync path raises.
fn take_proc(op: &'static str, id: i64) -> Result<Proc, SemaError> {
    PROCS.with(|p| {
        let mut procs = p.borrow_mut();
        match procs.get_mut(&id) {
            Some(slot @ ProcSlot::Available(_)) => {
                let ProcSlot::Available(pr) = std::mem::replace(slot, ProcSlot::CheckedOut) else {
                    unreachable!("just matched Available")
                };
                Ok(pr)
            }
            Some(ProcSlot::CheckedOut) => Err(busy_err(op, id)),
            Some(ProcSlot::Tombstone(msg)) => Err(tombstone_err(op, id, msg)),
            None => Err(missing_err(op, id)),
        }
    })
}

/// Peek the OS pid of an `Available` handle (for the cancel SIGKILL hook, built
/// before the `Proc` is checked out). `None` for any non-`Available` slot.
fn peek_pid(id: i64) -> Option<u32> {
    PROCS.with(|p| match p.borrow().get(&id) {
        Some(ProcSlot::Available(pr)) => Some(pr.child.id()),
        _ => None,
    })
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
    // Put the child in its own process group (pgid == pid) so a cancelled
    // async `proc/wait`/`proc/write-stdin` can SIGKILL the whole group — the
    // `sh -c "a | b"` leader AND the grandchildren it forks — via
    // `group_sigkill_abort`, mirroring shell's killpg teardown.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

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

/// Offload one blocking `proc/*` operation on the handle `id` through the
/// CHECKOUT pattern under the unified runtime (see `sqlite.rs`'s module doc
/// comment for the canonical writeup this mirrors): acquire the handle's
/// [`ResourceGate`] (creating it on first use), take the `Proc` out of its slot,
/// run `op` off the VM thread on the executor's blocking tier, then reinstall
/// the `Proc` and decode the result on the VM thread before releasing the gate.
/// A second `proc/*` op on a busy handle parks FIFO on the gate; a mid-flight
/// cancel tombstones the slot (best-effort — the blocking call keeps running
/// unattended) and runs `abort` (a process-group SIGKILL that also unsticks the
/// worker's blocking `child.wait()`).
fn checkout_runtime<T: Send + 'static>(
    op_name: &'static str,
    id: i64,
    op: impl FnOnce(&mut Proc) -> Result<T, String> + Send + 'static,
    decode: impl FnOnce(T) -> Value + 'static,
    abort: Option<Box<dyn FnOnce()>>,
) -> NativeResult {
    let kind = CompletionKind::try_from_raw(PROC_COMPLETION_KIND)
        .expect("proc completion kind is nonzero");
    let gate = PROC_GATES.with(|g| g.borrow().get(&id).cloned());
    checkout_external(CheckoutOp {
        op_name,
        kind,
        gate,
        store_gate: Box::new(move |gid| {
            PROC_GATES.with(|g| {
                g.borrow_mut().insert(id, gid);
            });
        }),
        remove_gate: Rc::new(move |gid| {
            PROC_GATES.with(|g| {
                let mut gates = g.borrow_mut();
                if gates.get(&id).map(ResourceGateHandle::id) == Some(gid) {
                    gates.remove(&id);
                }
            });
        }),
        take: Box::new(move || take_proc(op_name, id)),
        op: Box::new(op),
        reinstall: Box::new(move |pr| {
            PROCS.with(|p| {
                p.borrow_mut().insert(id, ProcSlot::Available(pr));
            });
        }),
        decode: Box::new(move |t| Ok(decode(t))),
        success_value: None,
        tombstone: Rc::new(move |msg| {
            PROCS.with(|p| {
                p.borrow_mut().insert(id, ProcSlot::Tombstone(msg));
            });
        }),
        abort,
        reclaim: None,
        terminal_on_success: false,
    })
}

/// The async-context `proc/wait`: offload `child.wait()` + the pump-thread joins
/// (the tail-buffering guarantee) through the checkout, reinstalling the reaped
/// `Proc` so a follow-up read — or a second `proc/wait` — still works, exactly
/// like the sync path. A cancelled wait SIGKILLs the child (unsticking the
/// worker) and tombstones the slot.
fn proc_wait_runtime(id: i64) -> NativeResult {
    let abort = peek_pid(id).map(group_sigkill_abort);
    checkout_runtime(
        "proc/wait",
        id,
        move |pr| {
            let code = pr
                .child
                .wait()
                .map(|s| s.code().unwrap_or(-1))
                .map_err(|e| format!("proc/wait: {e}"))?;
            if let Some(t) = pr.out_thread.take() {
                let _ = t.join();
            }
            if let Some(t) = pr.err_thread.take() {
                let _ = t.join();
            }
            Ok(code as i64)
        },
        Value::int,
        abort,
    )
}

/// The async-context `proc/write-stdin`: offload `sin.write_all(text) + flush()`
/// (a large write to a child not draining its stdin blocks in-kernel on the pipe
/// buffer). A cancelled write SIGKILLs the child (unsticking the blocked write)
/// and tombstones the slot.
fn proc_write_stdin_runtime(id: i64, text: Vec<u8>) -> NativeResult {
    let abort = peek_pid(id).map(group_sigkill_abort);
    checkout_runtime(
        "proc/write-stdin",
        id,
        move |pr| match pr.stdin.as_mut() {
            Some(sin) => sin
                .write_all(&text)
                .and_then(|_| sin.flush())
                .map_err(|e| format!("proc/write-stdin: {e}")),
            None => Err("proc/write-stdin: stdin already closed".to_string()),
        },
        |()| Value::nil(),
        abort,
    )
}

/// The async-context `proc/close`: mirrors the sync path up through `kill()` —
/// checked out synchronously on the VM thread (a signal send, not a wait), a
/// hard immediate `busy_err` on a handle a `proc/wait` already holds (never a
/// queue), a silent no-op on a tombstoned/missing handle — then offloads only
/// the blocking `child.wait()` + pump-thread joins through the checkout,
/// dropping the reaped `Proc` and freeing the slot + gate instead of
/// reinstalling it. Since the child is already killed, the cancel hook is just
/// the default tombstone (no extra abort).
fn proc_close_runtime(id: i64) -> NativeResult {
    enum CloseAction {
        Busy,
        Noop,
        Proceed,
    }
    let action = PROCS.with(|p| {
        let procs = p.borrow();
        match procs.get(&id) {
            Some(ProcSlot::CheckedOut) => CloseAction::Busy,
            Some(ProcSlot::Available(_)) => CloseAction::Proceed,
            Some(ProcSlot::Tombstone(_)) | None => CloseAction::Noop,
        }
    });
    let gate = PROC_GATES.with(|g| g.borrow().get(&id).cloned());
    if matches!(action, CloseAction::Busy) {
        return Err(busy_err("proc/close", id));
    }
    if prepare_terminal_gate(gate.as_ref(), "proc/close")? {
        if let Some(gate) = gate.as_ref() {
            PROC_GATES.with(|g| {
                let mut gates = g.borrow_mut();
                if gates.get(&id).map(ResourceGateHandle::id) == Some(gate.id()) {
                    gates.remove(&id);
                }
            });
        }
        if matches!(action, CloseAction::Noop) {
            return Ok(NativeOutcome::Return(Value::nil()));
        }
        let mut proc = take_proc("proc/close", id)?;
        let pid = proc.child.id();
        let _ = proc.child.kill();
        let kind = CompletionKind::try_from_raw(PROC_COMPLETION_KIND)
            .expect("proc completion kind is nonzero");
        return suspend_terminal_external(
            "proc/close",
            kind,
            proc,
            |proc| {
                let _ = proc.child.wait();
                if let Some(thread) = proc.out_thread.take() {
                    let _ = thread.join();
                }
                if let Some(thread) = proc.err_thread.take() {
                    let _ = thread.join();
                }
                Ok(())
            },
            move |_proc, result| {
                PROCS.with(|p| {
                    p.borrow_mut().remove(&id);
                });
                result.map_err(SemaError::Io)?;
                Ok(Value::nil())
            },
            Rc::new(move |msg| {
                PROCS.with(|p| {
                    p.borrow_mut().insert(id, ProcSlot::Tombstone(msg));
                });
            }),
            Some(group_sigkill_abort(pid)),
        );
    }
    match action {
        CloseAction::Busy => unreachable!("busy close returned above"),
        CloseAction::Noop => finish_terminal_gate(
            gate,
            Rc::new(move |gid| {
                PROC_GATES.with(|g| {
                    let mut gates = g.borrow_mut();
                    if gates.get(&id).map(ResourceGateHandle::id) == Some(gid) {
                        gates.remove(&id);
                    }
                });
            }),
            Ok(Value::nil()),
        ),
        CloseAction::Proceed => {
            PROCS.with(|p| {
                if let Some(ProcSlot::Available(proc)) = p.borrow_mut().get_mut(&id) {
                    let _ = proc.child.kill();
                }
            });
            let kind = CompletionKind::try_from_raw(PROC_COMPLETION_KIND)
                .expect("proc completion kind is nonzero");
            checkout_external(CheckoutOp {
                op_name: "proc/close",
                kind,
                gate,
                store_gate: Box::new(move |gid| {
                    PROC_GATES.with(|g| {
                        g.borrow_mut().insert(id, gid);
                    });
                }),
                remove_gate: Rc::new(move |gid| {
                    PROC_GATES.with(|g| {
                        let mut gates = g.borrow_mut();
                        if gates.get(&id).map(ResourceGateHandle::id) == Some(gid) {
                            gates.remove(&id);
                        }
                    });
                }),
                take: Box::new(move || take_proc("proc/close", id)),
                op: Box::new(move |pr: &mut Proc| {
                    let _ = pr.child.wait();
                    if let Some(t) = pr.out_thread.take() {
                        let _ = t.join();
                    }
                    if let Some(t) = pr.err_thread.take() {
                        let _ = t.join();
                    }
                    Ok(())
                }),
                // Drop the reaped `Proc` and free the slot + gate — `proc/close`
                // frees the handle rather than reinstalling it. Safe because we
                // only reach here with the slot `Available` (no gate waiter can
                // exist), and handle ids are never reused.
                reinstall: Box::new(move |_pr: Proc| {
                    PROCS.with(|p| {
                        p.borrow_mut().remove(&id);
                    });
                }),
                decode: Box::new(|()| Ok(Value::nil())),
                success_value: None,
                tombstone: Rc::new(move |msg| {
                    PROCS.with(|p| {
                        p.borrow_mut().insert(id, ProcSlot::Tombstone(msg));
                    });
                }),
                abort: None,
                reclaim: None,
                terminal_on_success: true,
            })
        }
    }
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

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        Caps::PROCESS,
        "proc/write-stdin",
        &[],
        |args| {
            check_arity!(args, "proc/write-stdin", 2);
            let id = handle(args, 0)?;
            let text = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            if in_runtime_quantum() {
                return proc_write_stdin_runtime(id, text.as_bytes().to_vec());
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
            .map(NativeOutcome::Return)
        },
    );

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
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "proc/wait", &[], |args| {
        check_arity!(args, "proc/wait", 1);
        let id = handle(args, 0)?;
        if in_runtime_quantum() {
            return proc_wait_runtime(id);
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
        Ok(NativeOutcome::Return(Value::int(
            status.code().unwrap_or(-1) as i64,
        )))
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
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "proc/close", &[], |args| {
        check_arity!(args, "proc/close", 1);
        let id = handle(args, 0)?;
        if in_runtime_quantum() {
            return proc_close_runtime(id);
        }
        let gate = PROC_GATES.with(|g| g.borrow().get(&id).cloned());
        if PROCS.with(|p| matches!(p.borrow().get(&id), Some(ProcSlot::CheckedOut))) {
            return Err(busy_err("proc/close", id));
        }
        prepare_terminal_gate(gate.as_ref(), "proc/close")?;
        PROCS.with(|p| {
            let mut procs = p.borrow_mut();
            if let Some(ProcSlot::Available(mut pr)) = procs.remove(&id) {
                let _ = pr.child.kill();
                let _ = pr.child.wait();
            }
        });
        finish_terminal_gate(
            gate,
            Rc::new(move |gate_id| {
                PROC_GATES.with(|g| {
                    let mut gates = g.borrow_mut();
                    if gates.get(&id).map(ResourceGateHandle::id) == Some(gate_id) {
                        gates.remove(&id);
                    }
                });
            }),
            Ok(Value::nil()),
        )
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
