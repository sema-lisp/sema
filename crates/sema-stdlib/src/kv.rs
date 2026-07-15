//! Key-value store primitives (`kv/*`).
//!
//! Stores live in a thread-local registry keyed by name. `kv/set`/`kv/delete`
//! are write-through by design: every mutation rewrites the ENTIRE store back
//! to disk as JSON before the call resolves, so a crash right after `kv/set`
//! returns must not lose the write.
//!
//! `kv/open` offloads its initial read+parse through `fs_offload` (io.rs) —
//! mirrors `db/open`'s shape (`sqlite.rs`): there is no existing store to
//! contend over, so the poller simply inserts the freshly-opened `KvStore`
//! into the registry on completion.
//!
//! `kv/set`/`kv/delete` use the CHECKOUT pattern (see `sqlite.rs`'s module
//! doc comment for the canonical writeup this mirrors): the registry slot is
//! `Available(KvStore)` / `CheckedOut` / `Tombstone(msg)`. Unlike the sqlite
//! checkout, the MUTATION itself (`data.insert`/`data.remove`) runs on the VM
//! thread the instant the checkout succeeds — never inside the offload — so
//! the write is observable to any other `kv/*` call as soon as this native
//! returns control to the scheduler, even though the call doesn't RESOLVE
//! until the flush completes. Only the flush (JSON encode + `std::fs::write`
//! of the whole store) moves to the I/O pool's blocking tier; the task stays
//! parked on `AwaitIo` until that flush completes — fire-and-forget would
//! violate the write-through durability contract. The offload's poller
//! reinstalls the (already-mutated) `KvStore` as `Available` and calls
//! `notify_io_complete()` so a sibling task queued on the SAME store can't
//! miss the wakeup. A second `kv/set`/`kv/delete` on a store that's already
//! checked out queues: its `IoHandle` re-attempts the checkout on every poll
//! (the `Acquire` phase) until the slot frees up, then mutates and spawns its
//! own flush — all under the one `IoHandle` the yield armed.
//!
//! `kv/get`/`kv/keys` are pure in-memory reads — no I/O — so they never
//! offload, but stay checkout-aware (`with_store`) so a store busy with an
//! in-flight flush reports a clear busy error instead of the registry entry
//! appearing to vanish.
//!
//! At top level (no scheduler) every builtin keeps today's synchronous shape
//! byte-for-byte.

use std::cell::RefCell;
use std::collections::HashMap;

use sema_core::{check_arity, in_async_context, IoHandle, IoPoll, SemaError, Value};

struct KvStore {
    path: String,
    data: serde_json::Map<String, serde_json::Value>,
}

// `kv/set`/`kv/delete`'s flush offload moves a whole `KvStore` onto the I/O
// pool's blocking tier and back. This compiles only if it stays `Send`; a
// future field addition that breaks it fails here, not with an opaque
// trait-bound error deep in `sema_io::io_spawn_blocking`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<KvStore>();
};

/// A registry slot. `CheckedOut` is the moment between a flush offload taking
/// the `KvStore` out (right after applying the mutation) and the poller
/// reinstalling it; every other `kv/*` op treats it as "busy, try again once
/// the in-flight flush resolves". `Tombstone` is terminal: set only when a
/// flush offload is cancelled mid-flight (the `KvStore` is stuck inside an
/// uncancellable background thread — see `spawn_flush`'s doc comment) or its
/// worker vanishes unexpectedly; `kv/close` is the only way to free a
/// tombstoned slot.
enum KvSlot {
    Available(KvStore),
    CheckedOut,
    Tombstone(String),
}

thread_local! {
    static KV_STORES: RefCell<HashMap<String, KvSlot>> = RefCell::new(HashMap::new());
}

/// `name` has never been `kv/open`ed (or was already `kv/close`d). Text
/// matches the pre-offload message verbatim — no `op` prefix — since every
/// sync-path call site rendered it this way.
fn missing_err(name: &str) -> SemaError {
    SemaError::eval(format!("kv store '{name}' not open"))
}

/// `op` was attempted while a flush offload had `name` checked out.
fn busy_err(op: &str, name: &str) -> SemaError {
    SemaError::eval(format!(
        "{op}: kv store '{name}' is busy — another kv/* call is in flight on it"
    ))
    .with_hint("wait for the in-flight kv/* call on this store to resolve before calling another")
}

/// `op` was attempted on a store whose in-flight flush was cancelled.
fn tombstone_err(op: &str, name: &str, reason: &str) -> SemaError {
    SemaError::eval(format!(
        "{op}: kv store '{name}' is no longer usable: {reason}"
    ))
}

/// Sync-path / non-offloaded accessor: look up `name` for an op that only
/// needs `&KvStore`, translating the other slot states into a clear,
/// `op`-specific error instead of ever panicking on the enum shape. Used both
/// by ops that never offload (`kv/get`, `kv/keys`) and — via
/// [`with_store_mut`] — by `kv/set`/`kv/delete`'s own top-level (non-async)
/// branch.
fn with_store<R>(
    op: &str,
    name: &str,
    f: impl FnOnce(&KvStore) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    KV_STORES.with(|s| {
        let stores = s.borrow();
        match stores.get(name) {
            Some(KvSlot::Available(store)) => f(store),
            Some(KvSlot::CheckedOut) => Err(busy_err(op, name)),
            Some(KvSlot::Tombstone(msg)) => Err(tombstone_err(op, name, msg)),
            None => Err(missing_err(name)),
        }
    })
}

/// Mutable twin of [`with_store`], for the sync `kv/set`/`kv/delete` path.
fn with_store_mut<R>(
    op: &str,
    name: &str,
    f: impl FnOnce(&mut KvStore) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    KV_STORES.with(|s| {
        let mut stores = s.borrow_mut();
        match stores.get_mut(name) {
            Some(KvSlot::Available(store)) => f(store),
            Some(KvSlot::CheckedOut) => Err(busy_err(op, name)),
            Some(KvSlot::Tombstone(msg)) => Err(tombstone_err(op, name, msg)),
            None => Err(missing_err(name)),
        }
    })
}

/// Read `path` (an empty store if it doesn't exist yet) and parse it as the
/// on-disk JSON object `kv/open` expects. Shared verbatim by the sync and
/// offloaded-async paths so a failure renders identically either way.
fn read_or_init_store(path: &str) -> Result<serde_json::Map<String, serde_json::Value>, SemaError> {
    if std::path::Path::new(path).exists() {
        let content =
            std::fs::read_to_string(path).map_err(|e| SemaError::Io(format!("kv/open: {e}")))?;
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content)
            .map_err(|e| SemaError::Io(format!("kv/open: malformed JSON in {path}: {e}")))
    } else {
        Ok(serde_json::Map::new())
    }
}

/// What a flush offload sends back to the poller: the reinstalled `KvStore`
/// (already mutated on the VM thread before the flush was ever spawned) plus
/// the flush's own owned `Send` result. Mirrors `sqlite.rs`'s `ConnOpOutcome`.
struct FlushOutcome {
    store: KvStore,
    result: Result<(), String>,
}

/// The two phases a `kv/set`/`kv/delete` `IoHandle` cycles through — same
/// shape as `sqlite.rs`'s `ConnPhase`/`proc.rs`'s `WaitPhase`. A caller that
/// finds the slot immediately `Available` still starts in `Acquire`; it
/// succeeds on the very first poll and falls through into `Running` in the
/// same tick, so there is exactly one code path for both the uncontended and
/// the queued case.
enum FlushPhase {
    /// Waiting for the slot to become `Available`. Re-checked every poll;
    /// mutates nothing until the checkout actually succeeds, so aborting here
    /// is a true no-op — nothing was ever taken out, nothing was ever
    /// written.
    Acquire,
    /// Holding the checkout; the (already-mutated) store's flush is running
    /// on the I/O pool. Resolves with the reinstalled `KvStore` plus the
    /// flush outcome; the mutation's own result travels alongside in
    /// `checkout_flush`'s `mutate_result` slot, not in this enum, since it
    /// never needs to be visible to the abort hook.
    Running(tokio::sync::oneshot::Receiver<FlushOutcome>),
}

/// Move `store`'s flush onto the I/O pool's blocking tier. Cancellation past
/// this point is best-effort by construction (the `KvStore` is inside a
/// `spawn_blocking` closure with no abort hook — the same tradeoff every
/// other `spawn_blocking`-based offload in this codebase accepts, see
/// `IoHandle::with_abort`'s doc comment): the caller marks the registry slot
/// `Tombstone` on abort so a later access errors clearly instead of the slot
/// staying `CheckedOut` forever with no one left to reinstall it, but the
/// write itself keeps running unattended on the worker — never torn down
/// mid-write, so the file is never left partially written by an abort.
fn spawn_flush(store: KvStore) -> tokio::sync::oneshot::Receiver<FlushOutcome> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let result = flush_store(&store).map_err(|e| e.to_string());
        let _ = tx.send(FlushOutcome { store, result });
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });
    rx
}

/// Offload one `kv/set`/`kv/delete` mutation+flush on the store named `name`
/// through the CHECKOUT pattern (see the module doc comment). `mutate` runs
/// synchronously ON THE VM THREAD the instant the checkout succeeds — never
/// inside the offload — so the in-memory write lands before anything is
/// handed to the I/O pool. `decode` turns `mutate`'s owned result into the
/// final `Value` once the flush resolves. Returns `Ok(nil)` after arming the
/// yield signal; the scheduler delivers the real value on resume.
fn checkout_flush<R: 'static>(
    op_name: &'static str,
    name: String,
    mutate: impl FnOnce(&mut KvStore) -> R + 'static,
    decode: impl FnOnce(R) -> Value + 'static,
) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let phase = Rc::new(RefCell::new(FlushPhase::Acquire));
    let phase_for_poll = phase.clone();
    let mut mutate_holder = Some(mutate);
    let mut decode_holder = Some(decode);
    // Set the instant the checkout succeeds (Acquire -> Running, still on the
    // VM thread), read back exactly once when the flush resolves. Lives
    // outside `FlushPhase` (unlike `sqlite.rs`'s checkout, which has no
    // separate VM-thread-only result to carry) since the abort hook never
    // needs to see it.
    let mut mutate_result: Option<R> = None;
    let name_for_poll = name.clone();

    let poll = move || -> IoPoll {
        loop {
            let is_acquire = matches!(&*phase_for_poll.borrow(), FlushPhase::Acquire);
            if is_acquire {
                // Owned Result so the KV_STORES borrow doesn't outlive the
                // match — the `Running` transition below needs its own
                // (non-overlapping) borrow of the same thread-local.
                let mut taken: Option<Result<KvStore, String>> = None;
                KV_STORES.with(|s| {
                    let mut stores = s.borrow_mut();
                    match stores.get_mut(&name_for_poll) {
                        Some(slot @ KvSlot::Available(_)) => {
                            let KvSlot::Available(mut store) =
                                std::mem::replace(slot, KvSlot::CheckedOut)
                            else {
                                unreachable!("just matched Available")
                            };
                            let mutate_fn = mutate_holder
                                .take()
                                .expect("checkout_flush's mutate is consumed exactly once");
                            mutate_result = Some(mutate_fn(&mut store));
                            taken = Some(Ok(store));
                        }
                        Some(KvSlot::CheckedOut) => {}
                        Some(KvSlot::Tombstone(msg)) => {
                            taken =
                                Some(Err(tombstone_err(op_name, &name_for_poll, msg).to_string()));
                        }
                        None => {
                            taken = Some(Err(missing_err(&name_for_poll).to_string()));
                        }
                    }
                });
                match taken {
                    None => return IoPoll::Pending,
                    Some(Err(msg)) => return IoPoll::Ready(Err(msg)),
                    Some(Ok(store)) => {
                        *phase_for_poll.borrow_mut() = FlushPhase::Running(spawn_flush(store));
                        // Fall through: poll the freshly spawned receiver
                        // immediately instead of wasting a scheduler tick.
                    }
                }
            } else {
                let mut phase_ref = phase_for_poll.borrow_mut();
                let FlushPhase::Running(rx) = &mut *phase_ref else {
                    unreachable!("Acquire handled above")
                };
                return match rx.try_recv() {
                    Err(TryRecvError::Empty) => IoPoll::Pending,
                    Ok(outcome) => {
                        drop(phase_ref);
                        KV_STORES.with(|s| {
                            s.borrow_mut()
                                .insert(name_for_poll.clone(), KvSlot::Available(outcome.store))
                        });
                        // MANDATORY lost-wakeup guard: a sibling queued on this
                        // same store (still in `Acquire`) may have polled
                        // Pending earlier in this scheduler sweep — without
                        // this it would park until an unrelated wakeup.
                        sema_core::notify_io_complete();
                        match outcome.result {
                            Ok(()) => {
                                let r = mutate_result
                                    .take()
                                    .expect("mutate_result set before the flush was spawned");
                                let decode_fn = decode_holder
                                    .take()
                                    .expect("checkout_flush's decode is consumed exactly once");
                                IoPoll::Ready(Ok(decode_fn(r)))
                            }
                            Err(msg) => IoPoll::Ready(Err(msg)),
                        }
                    }
                    Err(TryRecvError::Closed) => {
                        drop(phase_ref);
                        KV_STORES.with(|s| {
                            s.borrow_mut().insert(
                                name_for_poll.clone(),
                                KvSlot::Tombstone(
                                    "the flush worker terminated unexpectedly".to_string(),
                                ),
                            )
                        });
                        IoPoll::Ready(Err(format!("{op_name}: flush worker dropped")))
                    }
                };
            }
        }
    };

    let phase_for_abort = phase.clone();
    let name_for_abort = name;
    let io_handle = Rc::new(IoHandle::with_abort(poll, move || {
        // Acquire-phase abort: no-op — nothing was ever checked out or
        // mutated, the registry slot is exactly as another caller left it.
        // Running-phase abort: best-effort — see `spawn_flush`'s doc comment.
        if matches!(*phase_for_abort.borrow(), FlushPhase::Running(..)) {
            KV_STORES.with(|s| {
                s.borrow_mut().insert(
                    name_for_abort.clone(),
                    KvSlot::Tombstone(format!(
                        "{op_name} was cancelled while its flush was in flight; the store \
                         cannot be reclaimed — kv/close frees the handle"
                    )),
                );
            });
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(io_handle));
    Ok(Value::nil())
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "kv/open",
        &[1],
        |args| {
            check_arity!(args, "kv/open", 2);
            let name = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            let path = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                .to_string();

            if in_async_context() || sema_core::in_runtime_quantum() {
                let path_for_read = path.clone();
                let path_for_store = path.clone();
                let name_for_decode = name.clone();
                return crate::io::fs_offload(
                    move || read_or_init_store(&path_for_read).map_err(|e| e.to_string()),
                    move |data| {
                        KV_STORES.with(|s| {
                            s.borrow_mut().insert(
                                name_for_decode.clone(),
                                KvSlot::Available(KvStore {
                                    path: path_for_store.clone(),
                                    data,
                                }),
                            )
                        });
                        Value::string(&name_for_decode)
                    },
                );
            }

            let data = read_or_init_store(&path)?;
            KV_STORES.with(|s| {
                s.borrow_mut().insert(
                    name.clone(),
                    KvSlot::Available(KvStore {
                        path: path.clone(),
                        data,
                    }),
                )
            });
            Ok(Value::string(&name))
        },
    );

    crate::register_fn(env, "kv/get", |args| {
        check_arity!(args, "kv/get", 2);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let key = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        with_store("kv/get", name, |store| match store.data.get(key) {
            Some(v) => Ok(sema_core::json_to_value(v)),
            None => Ok(Value::nil()),
        })
    });

    // NOTE: kv/set and kv/delete write to the path stored at kv/open time without
    // re-checking allowed_paths. This is intentional — the path was validated at open,
    // and flush_store uses std::fs::write which recreates the file if deleted (correct
    // for a KV store). If path sandboxing needs to be stricter (e.g., re-validating on
    // every write to guard against the backing file being replaced with a symlink to an
    // outside path), the stored path should be canonicalized at open time and re-checked
    // in flush_store.
    crate::register_fn_gated(env, sandbox, sema_core::Caps::FS_WRITE, "kv/set", |args| {
        check_arity!(args, "kv/set", 3);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let key = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
            .to_string();
        let val = sema_core::value_to_json_lossy(&args[2]);

        if in_async_context() || sema_core::in_runtime_quantum() {
            let ret_val = args[2].clone();
            return checkout_flush(
                "kv/set",
                name,
                move |store| {
                    store.data.insert(key, val);
                },
                move |()| ret_val,
            );
        }

        with_store_mut("kv/set", &name, |store| {
            store.data.insert(key, val);
            flush_store(store)
        })?;
        Ok(args[2].clone())
    });

    crate::register_fn_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "kv/delete",
        |args| {
            check_arity!(args, "kv/delete", 2);
            let name = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            let key = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                .to_string();

            if in_async_context() || sema_core::in_runtime_quantum() {
                return checkout_flush(
                    "kv/delete",
                    name,
                    move |store| store.data.remove(&key).is_some(),
                    Value::bool,
                );
            }

            with_store_mut("kv/delete", &name, |store| {
                let existed = store.data.remove(&key).is_some();
                flush_store(store)?;
                Ok(Value::bool(existed))
            })
        },
    );

    crate::register_fn(env, "kv/keys", |args| {
        check_arity!(args, "kv/keys", 1);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        with_store("kv/keys", name, |store| {
            Ok(Value::list(
                store.data.keys().map(|k| Value::string(k)).collect(),
            ))
        })
    });

    // A store checked out by an in-flight flush errors instead of racing the
    // background write for the same `KvStore` (matches `db/close`); a
    // missing or already-tombstoned store is a silent no-op — `kv/close`
    // remains the documented way to free either.
    crate::register_fn(env, "kv/close", |args| {
        check_arity!(args, "kv/close", 1);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        KV_STORES.with(|s| {
            let mut stores = s.borrow_mut();
            match stores.get(name) {
                Some(KvSlot::CheckedOut) => return Err(busy_err("kv/close", name)),
                Some(KvSlot::Available(store)) => {
                    let _ = flush_store(store);
                }
                Some(KvSlot::Tombstone(_)) | None => {}
            }
            stores.remove(name);
            Ok(Value::nil())
        })
    });
}

fn flush_store(store: &KvStore) -> Result<(), SemaError> {
    let json = serde_json::to_string_pretty(&store.data)
        .map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    std::fs::write(&store.path, json).map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    Ok(())
}
