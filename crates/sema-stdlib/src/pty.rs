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
//! same CHECKOUT design `proc/wait`/`proc/write-stdin`/`proc/close` use
//! (`crate::runtime_offload::checkout_external`; see `proc.rs`'s module doc
//! comment): the registry slot (`PtySlot`) is `Available(Pty)` / `CheckedOut` /
//! `Tombstone(reason)`, guarded by a per-handle `ResourceGate` that serializes
//! concurrent ops FIFO. Each offload acquires the gate, takes the `Pty` (it is
//! `Send` — see the static assertion below) out of the slot, runs the blocking op
//! on the executor's blocking tier, then reinstalls the `Pty` (or, for
//! `pty/close`, drops it after reaping) and releases the gate. A busy handle
//! parks FIFO; a mid-flight cancel tombstones the slot (best-effort) and SIGKILLs
//! the pty child's process group. At top level every sync path is unchanged.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult, ResourceGateHandle};
use sema_core::{check_arity, in_runtime_quantum, Caps, SemaError, Value};

use crate::runtime_offload::{
    checkout_external, finish_terminal_gate, group_sigkill_abort, prepare_terminal_gate,
    suspend_terminal_external, CheckoutOp,
};

/// Completion-kind tag for `pty/*` external waits ("pty\0").
const PTY_COMPLETION_KIND: u64 = 0x7074_7900;

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
    /// Per-handle owning resource-gate capability, created lazily on the first
    /// offloaded op and reused for later ops (dropped on `pty/close`). The gate
    /// provides FIFO mutual exclusion for the checkout slot.
    static PTY_GATES: RefCell<HashMap<i64, ResourceGateHandle>> = RefCell::new(HashMap::new());
    /// Whether this thread's interpreter has an interpreter-teardown hook wired
    /// for the pty registry (C6) — see `proc.rs`'s `PROC_TEARDOWN_REGISTERED`.
    static PTY_TEARDOWN_REGISTERED: Cell<bool> = const { Cell::new(false) };
}

/// Register the interpreter-teardown hook for the pty registry against `ctx`
/// exactly once per interpreter (C6) — see `proc.rs`'s `ensure_teardown_hook`
/// (identical design, just for `Pty`). Called at `pty/spawn` so a pty child
/// spawned but never `pty/wait`ed/`pty/close`d is still reaped on drop.
fn ensure_teardown_hook(ctx: &sema_core::EvalContext) {
    if !PTY_TEARDOWN_REGISTERED.with(|c| c.replace(true)) {
        ctx.register_interpreter_teardown_hook(teardown_ptys);
    }
}

/// Interpreter-drop teardown for the pty registry (C6) — see `proc.rs`'s
/// `teardown_procs`. Every `Available` slot's child is SIGKILLed by process group
/// via the existing group-kill machinery (the pty child is its own
/// session/group leader) and reaped with a bounded `wait()`; dropping the `Pty`
/// closes the master and detaches the reader thread (EOF exits it). `CheckedOut`
/// slots hold no child. Gates are closed so any parked waiter fails fast.
fn teardown_ptys() {
    let slots: Vec<PtySlot> = PTYS.with(|p| p.borrow_mut().drain().map(|(_, slot)| slot).collect());
    for slot in slots {
        if let PtySlot::Available(mut pty) = slot {
            if let Some(pid) = pty.child.process_id() {
                group_sigkill_abort(pid)();
            }
            let _ = pty.child.kill();
            let _ = pty.child.wait();
        }
    }
    PTY_GATES.with(|g| {
        for (_, gate) in g.borrow_mut().drain() {
            let _ = gate.close();
        }
    });
    PTY_TEARDOWN_REGISTERED.with(|c| c.set(false));
}

/// Take `id`'s pty out of its slot once its gate is owned, marking the slot
/// `CheckedOut`. A tombstoned/missing/busy slot fails with the same clear text
/// the sync path raises.
fn take_pty(op: &'static str, id: i64) -> Result<Pty, SemaError> {
    PTYS.with(|p| {
        let mut ptys = p.borrow_mut();
        match ptys.get_mut(&id) {
            Some(slot @ PtySlot::Available(_)) => {
                let PtySlot::Available(pt) = std::mem::replace(slot, PtySlot::CheckedOut) else {
                    unreachable!("just matched Available")
                };
                Ok(pt)
            }
            Some(PtySlot::CheckedOut) => Err(busy_err(op, id)),
            Some(PtySlot::Tombstone(msg)) => Err(tombstone_err(op, id, msg)),
            None => Err(missing_err(op, id)),
        }
    })
}

/// Peek the OS pid of an `Available` handle (for the cancel SIGKILL hook, built
/// before the `Pty` is checked out). The pty child is its own session/group
/// leader, so a group SIGKILL reaps it. `None` for any non-`Available` slot or
/// a child that never reported a pid.
fn peek_pid(id: i64) -> Option<u32> {
    PTYS.with(|p| match p.borrow().get(&id) {
        Some(PtySlot::Available(pt)) => pt.child.process_id(),
        _ => None,
    })
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

fn spawn(ctx: &sema_core::EvalContext, args: &[Value]) -> Result<Value, SemaError> {
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
    // A live pty child now enters the registry: wire the interpreter-teardown
    // hook so it is reaped on drop even if `pty/wait`/`pty/close` is never called.
    ensure_teardown_hook(ctx);
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

/// Offload one blocking `pty/*` operation on the handle `id` through the
/// CHECKOUT pattern under the unified runtime — see `proc.rs`'s `checkout_runtime`
/// (identical design, just for `Pty`). Acquire the handle's [`ResourceGate`],
/// take the `Pty` out of its slot, run `op` off the VM thread on the executor's
/// blocking tier, reinstall the `Pty` and decode on the VM thread, then release
/// the gate. A busy handle parks FIFO; a cancel tombstones the slot and SIGKILLs
/// the pty child's process group (which also unsticks the worker).
fn checkout_runtime<T: Send + 'static>(
    op_name: &'static str,
    id: i64,
    op: impl FnOnce(&mut Pty) -> Result<T, String> + Send + 'static,
    decode: impl FnOnce(T) -> Value + 'static,
    abort: Option<Box<dyn FnOnce()>>,
) -> NativeResult {
    let kind =
        CompletionKind::try_from_raw(PTY_COMPLETION_KIND).expect("pty completion kind is nonzero");
    let gate = PTY_GATES.with(|g| g.borrow().get(&id).cloned());
    checkout_external(CheckoutOp {
        op_name,
        kind,
        gate,
        store_gate: Box::new(move |gid| {
            PTY_GATES.with(|g| {
                g.borrow_mut().insert(id, gid);
            });
        }),
        remove_gate: Rc::new(move |gid| {
            PTY_GATES.with(|g| {
                let mut gates = g.borrow_mut();
                if gates.get(&id).map(ResourceGateHandle::id) == Some(gid) {
                    gates.remove(&id);
                }
            });
        }),
        take: Box::new(move || take_pty(op_name, id)),
        op: Box::new(op),
        reinstall: Box::new(move |pt| {
            PTYS.with(|p| {
                p.borrow_mut().insert(id, PtySlot::Available(pt));
            });
        }),
        decode: Box::new(move |t| Ok(decode(t))),
        success_value: None,
        tombstone: Rc::new(move |msg| {
            PTYS.with(|p| {
                p.borrow_mut().insert(id, PtySlot::Tombstone(msg));
            });
        }),
        abort,
        reclaim: None,
        terminal_on_success: false,
    })
}

/// The async-context `pty/wait`: offload `child.wait()` + the reader-thread join
/// through the checkout, reinstalling the reaped `Pty` so a follow-up read — or
/// a second `pty/wait` — still works. A cancelled wait SIGKILLs the pty child
/// (unsticking the worker) and tombstones the slot.
fn pty_wait_runtime(id: i64) -> NativeResult {
    let abort = peek_pid(id).map(group_sigkill_abort);
    checkout_runtime(
        "pty/wait",
        id,
        move |pt| {
            let code = pt
                .child
                .wait()
                .map(|s| s.exit_code() as i32)
                .map_err(|e| format!("pty/wait: {e}"))?;
            if let Some(t) = pt.reader_thread.take() {
                let _ = t.join();
            }
            Ok(code as i64)
        },
        Value::int,
        abort,
    )
}

/// The async-context `pty/write`: offload `writer.write_all(text) + flush()` — a
/// child not draining the pty's input (or a slow master reader) can block a write
/// indefinitely. A cancelled write SIGKILLs the pty child and tombstones the slot.
fn pty_write_runtime(id: i64, text: Vec<u8>) -> NativeResult {
    let abort = peek_pid(id).map(group_sigkill_abort);
    checkout_runtime(
        "pty/write",
        id,
        move |pt| {
            pt.writer
                .write_all(&text)
                .and_then(|_| pt.writer.flush())
                .map_err(|e| format!("pty/write: {e}"))
        },
        |()| Value::nil(),
        abort,
    )
}

/// The async-context `pty/close`: mirrors the sync path up through `kill()` —
/// checked out synchronously on the VM thread (a signal send, not a wait), a hard
/// immediate `busy_err` on a handle a `pty/wait`/`pty/write` already holds (never
/// a queue), a silent no-op on a tombstoned/missing handle — then offloads only
/// the blocking `child.wait()` + reader-thread join through the checkout, dropping
/// the reaped `Pty` and freeing the slot + gate instead of reinstalling it. Since
/// the child is already killed, the cancel hook is just the default tombstone.
fn pty_close_runtime(id: i64) -> NativeResult {
    enum CloseAction {
        Busy,
        Noop,
        Proceed,
    }
    let action = PTYS.with(|p| {
        let ptys = p.borrow();
        match ptys.get(&id) {
            Some(PtySlot::CheckedOut) => CloseAction::Busy,
            Some(PtySlot::Available(_)) => CloseAction::Proceed,
            Some(PtySlot::Tombstone(_)) | None => CloseAction::Noop,
        }
    });
    let gate = PTY_GATES.with(|g| g.borrow().get(&id).cloned());
    if matches!(action, CloseAction::Busy) {
        return Err(busy_err("pty/close", id));
    }
    if prepare_terminal_gate(gate.as_ref(), "pty/close")? {
        if let Some(gate) = gate.as_ref() {
            PTY_GATES.with(|g| {
                let mut gates = g.borrow_mut();
                if gates.get(&id).map(ResourceGateHandle::id) == Some(gate.id()) {
                    gates.remove(&id);
                }
            });
        }
        if matches!(action, CloseAction::Noop) {
            return Ok(NativeOutcome::Return(Value::nil()));
        }
        let pid = peek_pid(id);
        let mut pty = take_pty("pty/close", id)?;
        let _ = pty.child.kill();
        let kind = CompletionKind::try_from_raw(PTY_COMPLETION_KIND)
            .expect("pty completion kind is nonzero");
        return suspend_terminal_external(
            "pty/close",
            kind,
            pty,
            |pty| {
                let _ = pty.child.wait();
                if let Some(thread) = pty.reader_thread.take() {
                    let _ = thread.join();
                }
                Ok(())
            },
            move |_pty, result| {
                PTYS.with(|p| {
                    p.borrow_mut().remove(&id);
                });
                result.map_err(SemaError::Io)?;
                Ok(Value::nil())
            },
            Rc::new(move |msg| {
                PTYS.with(|p| {
                    p.borrow_mut().insert(id, PtySlot::Tombstone(msg));
                });
            }),
            pid.map(group_sigkill_abort),
        );
    }
    match action {
        CloseAction::Busy => unreachable!("busy close returned above"),
        CloseAction::Noop => finish_terminal_gate(
            gate,
            Rc::new(move |gid| {
                PTY_GATES.with(|g| {
                    let mut gates = g.borrow_mut();
                    if gates.get(&id).map(ResourceGateHandle::id) == Some(gid) {
                        gates.remove(&id);
                    }
                });
            }),
            Ok(Value::nil()),
        ),
        CloseAction::Proceed => {
            PTYS.with(|p| {
                if let Some(PtySlot::Available(pty)) = p.borrow_mut().get_mut(&id) {
                    let _ = pty.child.kill();
                }
            });
            let kind = CompletionKind::try_from_raw(PTY_COMPLETION_KIND)
                .expect("pty completion kind is nonzero");
            checkout_external(CheckoutOp {
                op_name: "pty/close",
                kind,
                gate,
                store_gate: Box::new(move |gid| {
                    PTY_GATES.with(|g| {
                        g.borrow_mut().insert(id, gid);
                    });
                }),
                remove_gate: Rc::new(move |gid| {
                    PTY_GATES.with(|g| {
                        let mut gates = g.borrow_mut();
                        if gates.get(&id).map(ResourceGateHandle::id) == Some(gid) {
                            gates.remove(&id);
                        }
                    });
                }),
                take: Box::new(move || take_pty("pty/close", id)),
                op: Box::new(move |pt: &mut Pty| {
                    let _ = pt.child.wait();
                    if let Some(t) = pt.reader_thread.take() {
                        let _ = t.join();
                    }
                    Ok(())
                }),
                // Drop the reaped `Pty` and free the slot + gate — `pty/close`
                // frees the handle. Safe because we only reach here with the slot
                // `Available` (no gate waiter can exist) and ids are never reused.
                reinstall: Box::new(move |_pt: Pty| {
                    PTYS.with(|p| {
                        p.borrow_mut().remove(&id);
                    });
                }),
                decode: Box::new(|()| Ok(Value::nil())),
                success_value: None,
                tombstone: Rc::new(move |msg| {
                    PTYS.with(|p| {
                        p.borrow_mut().insert(id, PtySlot::Tombstone(msg));
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
    crate::register_fn_gated_ctx(env, sandbox, Caps::PROCESS, "pty/spawn", spawn);

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

    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "pty/write", &[], |args| {
        check_arity!(args, "pty/write", 2);
        let id = handle(args, 0)?;
        let text = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        if in_runtime_quantum() {
            return pty_write_runtime(id, text.as_bytes().to_vec());
        }
        with_pty("pty/write", id, |pt| {
            pt.writer
                .write_all(text.as_bytes())
                .and_then(|_| pt.writer.flush())
                .map_err(|e| SemaError::Io(format!("pty/write: {e}")))?;
            Ok(Value::nil())
        })
        .map(NativeOutcome::Return)
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
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "pty/wait", &[], |args| {
        check_arity!(args, "pty/wait", 1);
        let id = handle(args, 0)?;
        if in_runtime_quantum() {
            return pty_wait_runtime(id);
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
        Ok(NativeOutcome::Return(Value::int(status.exit_code() as i64)))
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
    crate::register_runtime_fn_path_gated(env, sandbox, Caps::PROCESS, "pty/close", &[], |args| {
        check_arity!(args, "pty/close", 1);
        let id = handle(args, 0)?;
        if in_runtime_quantum() {
            return pty_close_runtime(id);
        }
        let gate = PTY_GATES.with(|g| g.borrow().get(&id).cloned());
        if PTYS.with(|p| matches!(p.borrow().get(&id), Some(PtySlot::CheckedOut))) {
            return Err(busy_err("pty/close", id));
        }
        prepare_terminal_gate(gate.as_ref(), "pty/close")?;
        PTYS.with(|p| {
            let mut ptys = p.borrow_mut();
            if let Some(PtySlot::Available(mut pt)) = ptys.remove(&id) {
                let _ = pt.child.kill();
                let _ = pt.child.wait();
            }
        });
        finish_terminal_gate(
            gate,
            Rc::new(move |gate_id| {
                PTY_GATES.with(|g| {
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

    /// C6 guard: `ensure_teardown_hook` wires exactly one hook (idempotent), and
    /// firing it drains the pty registry and resets the flag. A `Tombstone` slot
    /// stands in for a live pty so the drain is asserted without pty hardware —
    /// the real kill+reap is covered by `proc_pty_async_test.rs`.
    #[test]
    fn teardown_hook_registered_exactly_once_and_drains_registry() {
        let ctx = EvalContext::new();
        PTYS.with(|p| p.borrow_mut().clear());
        PTY_TEARDOWN_REGISTERED.with(|c| c.set(false));

        PTYS.with(|p| {
            p.borrow_mut()
                .insert(99, PtySlot::Tombstone("guard".to_string()))
        });

        assert!(!PTY_TEARDOWN_REGISTERED.with(Cell::get));
        ensure_teardown_hook(&ctx);
        assert!(
            PTY_TEARDOWN_REGISTERED.with(Cell::get),
            "ensure_teardown_hook must register the interpreter hook"
        );
        ensure_teardown_hook(&ctx); // second call is a no-op

        assert!(ctx.try_run_interpreter_teardown_hooks());
        assert!(
            PTYS.with(|p| p.borrow().is_empty()),
            "teardown must drain the pty registry"
        );
        assert!(
            !PTY_TEARDOWN_REGISTERED.with(Cell::get),
            "teardown must reset the hook flag so a fresh interpreter re-registers"
        );
    }
}
