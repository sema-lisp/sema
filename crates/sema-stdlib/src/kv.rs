//! Key-value store primitives (`kv/*`).
//!
//! Stores live in a thread-local registry keyed by name. `kv/set`/`kv/delete`
//! are write-through by design: every mutation rewrites the ENTIRE store back
//! to disk as JSON before the call resolves, so a crash right after `kv/set`
//! returns must not lose the write.
//!
//! `kv/open` offloads its initial read+parse onto the executor's blocking tier
//! as a plain External wait (`external_io_interruptible_try`) — mirrors
//! `db/open`'s shape (`sqlite.rs`): there is no existing store to contend over,
//! so the decoder simply inserts the freshly-opened `KvStore` into the registry
//! on completion.
//!
//! `kv/set`/`kv/delete` use the CHECKOUT pattern under the unified runtime via
//! `runtime_offload::checkout_external` (see `sqlite.rs`'s module doc comment for
//! the canonical writeup this mirrors): the registry slot is `Available(KvStore)`
//! / `CheckedOut` / `Tombstone(msg)`, guarded by a per-store `ResourceGate` that
//! serializes concurrent mutations FIFO. On acquire the `KvStore` is taken out of
//! the slot; the mutation (`data.insert`/`data.remove`) AND the whole-store flush
//! (JSON encode + `std::fs::write`) both run on the blocking tier while the gate
//! is held, then the store is reinstalled and the gate released. The task stays
//! parked until the flush completes — fire-and-forget would violate the
//! write-through durability contract. Because the gate serializes access, no
//! other `kv/*` call observes the store mid-mutation: a second `kv/set`/`kv/delete`
//! on a busy store parks FIFO on the gate; a concurrent `kv/get`/`kv/keys` sees
//! `CheckedOut` and reports a clear busy error. A mid-flight cancel tombstones the
//! slot (best-effort — the write completes unattended, never torn mid-write) and
//! releases the gate so a queued sibling fails fast.
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
use std::rc::Rc;

use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult, ResourceGateId};
use sema_core::{check_arity, in_runtime_quantum, SemaError, Value};

use crate::runtime_offload::{checkout_external, CheckoutOp};

/// Completion-kind tag for `kv/*` external waits ("kv\0\0").
const KV_COMPLETION_KIND: u64 = 0x6b76_0000;

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

/// A registry slot. `CheckedOut` is the moment between a checkout taking the
/// `KvStore` out and the decoder reinstalling it after the mutation+flush; every
/// other `kv/*` op treats it as "busy, try again once the in-flight flush
/// resolves". `Tombstone` is terminal: set only when a flush is cancelled
/// mid-flight (the `KvStore` is stuck inside an uncancellable blocking worker —
/// best-effort cancellation) or its worker vanishes unexpectedly; `kv/close` is
/// the only way to free a tombstoned slot.
enum KvSlot {
    Available(KvStore),
    CheckedOut,
    Tombstone(String),
}

thread_local! {
    static KV_STORES: RefCell<HashMap<String, KvSlot>> = RefCell::new(HashMap::new());
    /// Per-store [`ResourceGateId`], created lazily on the first offloaded
    /// mutation on a store and reused for its later mutations (dropped on
    /// `kv/close`). The gate provides FIFO mutual exclusion for the checkout slot.
    static KV_GATES: RefCell<HashMap<String, ResourceGateId>> = RefCell::new(HashMap::new());
}

/// Take `name`'s store out of its slot once its gate is owned. A tombstoned or
/// missing slot (a prior flush cancelled mid-flight) fails clearly.
fn take_store(op_name: &'static str, name: &str) -> Result<KvStore, SemaError> {
    KV_STORES.with(|s| {
        let mut stores = s.borrow_mut();
        match stores.get_mut(name) {
            Some(slot @ KvSlot::Available(_)) => {
                let KvSlot::Available(store) = std::mem::replace(slot, KvSlot::CheckedOut) else {
                    unreachable!("just matched Available")
                };
                Ok(store)
            }
            Some(KvSlot::CheckedOut) => Err(busy_err(op_name, name)),
            Some(KvSlot::Tombstone(msg)) => Err(tombstone_err(op_name, name, msg)),
            None => Err(missing_err(name)),
        }
    })
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

/// Offload one `kv/set`/`kv/delete` mutation+flush on the store named `name`
/// through the CHECKOUT pattern under the unified runtime (see the module doc
/// comment). Acquire the store's [`ResourceGate`] (creating it on first use),
/// take the `KvStore` out of the slot, then — on the executor's blocking tier —
/// apply `mutate` and flush the whole store to disk before resolving (the
/// write-through durability contract), then reinstall the store and release the
/// gate. `mutate` runs on the worker while the gate is held, so no other `kv/*`
/// call can observe the store mid-flush (they see `CheckedOut` and queue/error).
/// A mid-flight cancel tombstones the slot (best-effort — the write completes
/// unattended, never torn down mid-write) and releases the gate.
fn checkout_runtime<R: Send + 'static>(
    op_name: &'static str,
    name: String,
    mutate: impl FnOnce(&mut KvStore) -> R + Send + 'static,
    decode: impl FnOnce(R) -> Value + 'static,
    success_value: Option<Value>,
) -> NativeResult {
    let kind =
        CompletionKind::try_from_raw(KV_COMPLETION_KIND).expect("kv completion kind is nonzero");
    let gate = KV_GATES.with(|g| g.borrow().get(&name).copied());
    let n_take = name.clone();
    let n_reinstall = name.clone();
    let n_tomb = name.clone();
    let n_store = name;
    checkout_external(CheckoutOp {
        op_name,
        kind,
        gate,
        store_gate: Box::new(move |id| {
            KV_GATES.with(|g| {
                g.borrow_mut().insert(n_store, id);
            });
        }),
        take: Box::new(move || take_store(op_name, &n_take)),
        op: Box::new(move |store: &mut KvStore| {
            let r = mutate(store);
            flush_store(store).map(|()| r).map_err(|e| e.to_string())
        }),
        reinstall: Box::new(move |store| {
            KV_STORES.with(|s| {
                s.borrow_mut().insert(n_reinstall, KvSlot::Available(store));
            });
        }),
        decode: Box::new(move |r| Ok(decode(r))),
        success_value,
        tombstone: Rc::new(move |msg| {
            KV_STORES.with(|s| {
                s.borrow_mut()
                    .insert(n_tomb.clone(), KvSlot::Tombstone(msg));
            });
        }),
        abort: None,
    })
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    crate::register_runtime_fn_path_gated(
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

            if in_runtime_quantum() {
                let kind = CompletionKind::try_from_raw(KV_COMPLETION_KIND)
                    .expect("kv completion kind is nonzero");
                let path_for_read = path.clone();
                let path_for_store = path;
                let name_for_decode = name;
                return crate::runtime_offload::external_io_interruptible_try(
                    "kv/open",
                    kind,
                    "kv/open",
                    move |data: serde_json::Map<String, serde_json::Value>| {
                        KV_STORES.with(|s| {
                            s.borrow_mut().insert(
                                name_for_decode.clone(),
                                KvSlot::Available(KvStore {
                                    path: path_for_store.clone(),
                                    data,
                                }),
                            )
                        });
                        Ok(Value::string(&name_for_decode))
                    },
                    move || async move { read_or_init_store(&path_for_read).map_err(|e| e.to_string()) },
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
            Ok(NativeOutcome::Return(Value::string(&name)))
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
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "kv/set",
        &[],
        |args| {
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

            if in_runtime_quantum() {
                // The stored value is returned verbatim — carried as a traced
                // `success_value`, not captured in `decode` (which is not traced).
                let ret_val = args[2].clone();
                return checkout_runtime(
                    "kv/set",
                    name,
                    move |store| {
                        store.data.insert(key, val);
                    },
                    |()| Value::nil(),
                    Some(ret_val),
                );
            }

            with_store_mut("kv/set", &name, |store| {
                store.data.insert(key, val);
                flush_store(store)
            })?;
            Ok(NativeOutcome::Return(args[2].clone()))
        },
    );

    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "kv/delete",
        &[],
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

            if in_runtime_quantum() {
                return checkout_runtime(
                    "kv/delete",
                    name,
                    move |store| store.data.remove(&key).is_some(),
                    Value::bool,
                    None,
                );
            }

            with_store_mut("kv/delete", &name, |store| {
                let existed = store.data.remove(&key).is_some();
                flush_store(store)?;
                Ok(Value::bool(existed))
            })
            .map(NativeOutcome::Return)
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
    // remains the documented way to free either. The store's resource gate is
    // dropped here too; a successful `kv/close` implies the gate is idle (a busy
    // gate means CheckedOut, which errors above), so no waiter is stranded.
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
            Ok(())
        })?;
        KV_GATES.with(|g| {
            g.borrow_mut().remove(name);
        });
        Ok(Value::nil())
    });
}

fn flush_store(store: &KvStore) -> Result<(), SemaError> {
    let json = serde_json::to_string_pretty(&store.data)
        .map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    std::fs::write(&store.path, json).map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    Ok(())
}
