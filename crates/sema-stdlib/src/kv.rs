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
//! closes the gate so every queued sibling wakes with a terminal error.
//!
//! `kv/get`/`kv/keys` are pure in-memory reads — no I/O — so they never
//! offload, but stay checkout-aware (`with_store`) so a store busy with an
//! in-flight flush reports a clear busy error instead of the registry entry
//! appearing to vanish.
//!
//! **Persistence bounds ([`KvBounds`]).** Every store is bounded before any
//! blocking work is dispatched. `kv/open` preflights the backing file's size on
//! the VM thread (a metadata stat) and then reads it through a capped
//! `Read::take` so an oversized store is rejected without ever allocating its
//! whole contents. `kv/set` rejects — pre-dispatch, with the store byte-for-byte
//! intact — a new key past [`KV_MAX_ITEMS`] or a value whose serialized form
//! alone exceeds [`KV_MAX_STORE_BYTES`], and [`flush_store`] re-checks the
//! serialized whole-store size as a final gate before it ever touches disk.
//! Because these caps make each op finite work, they — not a wall-clock timer —
//! are the finite-work bound for the flush (R09B `QUARANTINED-BOUNDED`): the JSON
//! backend is a plain `std::fs::write`, which exposes no interrupt handle, so a
//! mid-flush cancel cannot abort the write. That is R09A's narrowed contract: the
//! interruptible edges are the gate wait, `kv/close`, and foreign-runtime close;
//! a mid-op cancel falls back to tombstoning the slot and discarding the eventual
//! (never torn) write — not a faked abort.
//!
//! At top level (no scheduler) every builtin keeps today's synchronous shape
//! byte-for-byte for in-cap data.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Read as _;
use std::rc::Rc;

use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult, ResourceGateHandle};
use sema_core::{check_arity, in_runtime_quantum, SemaError, Value};

use crate::runtime_offload::{
    checkout_external, finish_terminal_gate, prepare_terminal_gate, CheckoutOp,
};

/// Completion-kind tag for `kv/*` external waits ("kv\0\0").
const KV_COMPLETION_KIND: u64 = 0x6b76_0000;

/// Hard ceiling on the bytes a store may occupy: the size `kv/open` will load
/// and the size a mutation's serialized whole-store JSON may reach before the
/// flush is refused. An oversized backing file is rejected by a metadata stat
/// plus a capped read (never allocating the whole file); an over-cap mutation is
/// rejected before the write reaches disk. Because the JSON backend is a plain
/// `std::fs::write` with no interrupt handle, this byte cap — not a wall-clock
/// timer — is the finite-work bound that keeps a checked-out flush bounded
/// (R09B `QUARANTINED-BOUNDED`).
const KV_MAX_STORE_BYTES: u64 = 64 * 1024 * 1024;
/// Hard ceiling on the number of items one store may hold. A `kv/set` that would
/// add a *new* key past this count is rejected pre-dispatch, before the write job
/// is enqueued, with the store left intact.
const KV_MAX_ITEMS: usize = 1_000_000;

/// The byte/item caps applied to a store, captured on the VM thread before any
/// blocking work dispatches and carried by value onto the worker (so a worker
/// enforces the same caps the VM thread admitted against — never a later
/// thread-local read).
#[derive(Clone, Copy)]
struct KvBounds {
    max_store_bytes: u64,
    max_items: usize,
}

/// The shipped hard ceilings. `effective_bounds` lowers these by any per-thread
/// override but never raises them.
const KV_RUNTIME_BOUNDS: KvBounds = KvBounds {
    max_store_bytes: KV_MAX_STORE_BYTES,
    max_items: KV_MAX_ITEMS,
};

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
    /// Per-store owning resource-gate capability, created lazily on the first
    /// offloaded mutation and reused for later mutations (dropped on
    /// `kv/close`). The gate provides FIFO mutual exclusion for the checkout slot.
    static KV_GATES: RefCell<HashMap<String, ResourceGateHandle>> = RefCell::new(HashMap::new());
    /// Optional per-thread lowered store bounds (clamped to the hard ceilings,
    /// never raised). `None` uses [`KV_RUNTIME_BOUNDS`]. The seam the regression
    /// suite drives so an over-cap test needs neither a 64 MiB file nor a million
    /// keys — mirrors `sqlite::DB_RESULT_CAPS_OVERRIDE`.
    static KV_BOUNDS_OVERRIDE: Cell<Option<KvBounds>> = const { Cell::new(None) };
}

/// The effective store bounds for the current call: the module hard ceilings,
/// lowered by any per-thread override (never raised above the ceilings). Read on
/// the VM thread before dispatch, then captured by value into the offloaded op.
fn effective_bounds() -> KvBounds {
    KV_BOUNDS_OVERRIDE
        .with(Cell::get)
        .map_or(KV_RUNTIME_BOUNDS, |over| KvBounds {
            max_store_bytes: over.max_store_bytes.min(KV_RUNTIME_BOUNDS.max_store_bytes),
            max_items: over.max_items.min(KV_RUNTIME_BOUNDS.max_items),
        })
}

/// Lower the per-thread KV store bounds (clamped to the hard ceilings) for
/// subsequent `kv/open`/`kv/set` calls on this thread, or clear the override with
/// `None`. The hard ceilings are unaffected; this is the seam a bounded-store
/// caller (and the regression suite) drives. Mirrors `set_db_result_caps_override`.
pub fn set_kv_bounds_override(bounds: Option<(u64, usize)>) {
    KV_BOUNDS_OVERRIDE.with(|cell| {
        cell.set(bounds.map(|(max_store_bytes, max_items)| KvBounds {
            max_store_bytes,
            max_items,
        }));
    });
}

/// A store's on-disk load or serialized-flush size exceeded the byte cap. Every
/// byte-cap rejection renders "…kv store limit" so callers can match one string.
fn store_bytes_cap_err(op: &str, subject: &str, actual: u64, limit: u64) -> SemaError {
    SemaError::eval(format!(
        "{op}: {subject} is {actual} bytes, over the {limit}-byte kv store limit"
    ))
    .with_hint("split the data across multiple kv stores")
}

/// A `kv/set` would push a store past its item cap.
fn item_cap_err(op: &str, name: &str, count: usize, limit: usize) -> SemaError {
    SemaError::eval(format!(
        "{op}: kv store '{name}' already holds {count} items, at the {limit}-item kv store limit"
    ))
    .with_hint("delete unused keys or split the data across multiple kv stores")
}

/// Reject a `kv/set` value whose serialized form alone exceeds the whole-store
/// byte cap. Touches no store state, so it is safe to run pre-dispatch even while
/// a sibling has the store checked out (it never returns a spurious busy error).
/// `val` is the already-JSON-encoded incoming value.
fn check_value_bytes(
    op: &str,
    name: &str,
    key: &str,
    val: &serde_json::Value,
    bounds: KvBounds,
) -> Result<(), SemaError> {
    let val_bytes = serde_json::to_vec(val).map_or(0, |v| v.len() as u64);
    if val_bytes > bounds.max_store_bytes {
        return Err(store_bytes_cap_err(
            op,
            &format!("value for key '{key}' on kv store '{name}'"),
            val_bytes,
            bounds.max_store_bytes,
        ));
    }
    Ok(())
}

/// Reject inserting a *new* key past the item cap. Runs only where the store is
/// exclusively owned — the sync `with_store_mut` path, or the checkout worker
/// after `take` — so it never observes a mid-flight `CheckedOut` slot and never
/// races the FIFO gate. It runs before the mutation, so an over-cap rejection
/// leaves the store byte-for-byte intact (the checkout worker carries the
/// unchanged store back and reinstalls it `Available`).
fn check_item_cap(
    op: &str,
    name: &str,
    store: &KvStore,
    key: &str,
    bounds: KvBounds,
) -> Result<(), SemaError> {
    if !store.data.contains_key(key) && store.data.len() >= bounds.max_items {
        return Err(item_cap_err(op, name, store.data.len(), bounds.max_items));
    }
    Ok(())
}

/// Metadata-only pre-dispatch admission for `kv/open`: reject an oversized backing
/// file on the VM thread before the read job is enqueued, allocating nothing. A
/// missing file (fresh store) or a stat error is admitted here and handled by the
/// capped read in [`read_or_init_store`].
fn preflight_store_size(op: &str, path: &str, bounds: KvBounds) -> Result<(), SemaError> {
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.is_file() && metadata.len() > bounds.max_store_bytes {
            return Err(store_bytes_cap_err(
                op,
                &format!("store {path}"),
                metadata.len(),
                bounds.max_store_bytes,
            ));
        }
    }
    Ok(())
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
/// offloaded-async paths so a failure renders identically either way. The read is
/// bounded: a metadata stat rejects an oversized file, and the file is then read
/// through a `Read::take` capped at `max_store_bytes + 1` so a file that grew
/// past the stat (TOCTOU) or a special file whose metadata under-reports its
/// length still cannot allocate more than the cap before it is rejected.
fn read_or_init_store(
    path: &str,
    bounds: KvBounds,
) -> Result<serde_json::Map<String, serde_json::Value>, SemaError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::Map::new());
        }
        Err(e) => return Err(SemaError::Io(format!("kv/open: {e}"))),
    };
    if let Ok(metadata) = file.metadata() {
        if metadata.len() > bounds.max_store_bytes {
            return Err(store_bytes_cap_err(
                "kv/open",
                &format!("store {path}"),
                metadata.len(),
                bounds.max_store_bytes,
            ));
        }
    }
    let mut content = String::new();
    file.take(bounds.max_store_bytes.saturating_add(1))
        .read_to_string(&mut content)
        .map_err(|e| SemaError::Io(format!("kv/open: {e}")))?;
    if content.len() as u64 > bounds.max_store_bytes {
        return Err(store_bytes_cap_err(
            "kv/open",
            &format!("store {path}"),
            content.len() as u64,
            bounds.max_store_bytes,
        ));
    }
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content)
        .map_err(|e| SemaError::Io(format!("kv/open: malformed JSON in {path}: {e}")))
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
/// unattended, never torn down mid-write) and closes the gate.
fn checkout_runtime<R: Send + 'static>(
    op_name: &'static str,
    name: String,
    admit: impl FnOnce(&KvStore) -> Result<(), String> + Send + 'static,
    mutate: impl FnOnce(&mut KvStore) -> R + Send + 'static,
    decode: impl FnOnce(R) -> Value + 'static,
    success_value: Option<Value>,
    bounds: KvBounds,
) -> NativeResult {
    let kind =
        CompletionKind::try_from_raw(KV_COMPLETION_KIND).expect("kv completion kind is nonzero");
    let gate = KV_GATES.with(|g| g.borrow().get(&name).cloned());
    let n_take = name.clone();
    let n_reinstall = name.clone();
    let n_tomb = name.clone();
    let n_remove = name.clone();
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
        remove_gate: Rc::new(move |id| {
            KV_GATES.with(|g| {
                let mut gates = g.borrow_mut();
                if gates.get(&n_remove).map(ResourceGateHandle::id) == Some(id) {
                    gates.remove(&n_remove);
                }
            });
        }),
        take: Box::new(move || take_store(op_name, &n_take)),
        op: Box::new(move |store: &mut KvStore| {
            // Item-count admission runs on the exclusively-owned store before the
            // mutation, so an over-cap rejection reinstalls the store unchanged.
            admit(store)?;
            let r = mutate(store);
            flush_store(store, bounds).map(|()| r).map_err(|e| e.to_string())
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
        reclaim: None,
        terminal_on_success: false,
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

            let bounds = effective_bounds();

            if in_runtime_quantum() {
                // Pre-dispatch admission: reject an oversized backing file on the
                // VM thread (metadata stat, no allocation) before enqueueing the
                // read job; the worker's capped read is the TOCTOU/growing-file
                // backstop.
                preflight_store_size("kv/open", &path, bounds)?;
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
                    move || async move {
                        read_or_init_store(&path_for_read, bounds).map_err(|e| e.to_string())
                    },
                );
            }

            let data = read_or_init_store(&path, bounds)?;
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

            let bounds = effective_bounds();
            // Value-size admission needs no store access, so it is safe
            // pre-dispatch even while a sibling has the store checked out; the
            // item-count admission runs on the exclusively-owned store (worker /
            // `with_store_mut`), so it neither races the FIFO gate nor mutates
            // before rejecting.
            check_value_bytes("kv/set", &name, &key, &val, bounds)?;

            if in_runtime_quantum() {
                // The stored value is returned verbatim — carried as a traced
                // `success_value`, not captured in `decode` (which is not traced).
                let ret_val = args[2].clone();
                let name_admit = name.clone();
                let key_admit = key.clone();
                return checkout_runtime(
                    "kv/set",
                    name,
                    move |store: &KvStore| {
                        check_item_cap("kv/set", &name_admit, store, &key_admit, bounds)
                            .map_err(|e| e.to_string())
                    },
                    move |store| {
                        store.data.insert(key, val);
                    },
                    |()| Value::nil(),
                    Some(ret_val),
                    bounds,
                );
            }

            with_store_mut("kv/set", &name, |store| {
                check_item_cap("kv/set", &name, store, &key, bounds)?;
                store.data.insert(key, val);
                flush_store(store, bounds)
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

            let bounds = effective_bounds();

            if in_runtime_quantum() {
                return checkout_runtime(
                    "kv/delete",
                    name,
                    // Delete only shrinks the store — no admission needed.
                    |_: &KvStore| Ok(()),
                    move |store| store.data.remove(&key).is_some(),
                    Value::bool,
                    None,
                    bounds,
                );
            }

            with_store_mut("kv/delete", &name, |store| {
                let existed = store.data.remove(&key).is_some();
                flush_store(store, bounds)?;
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
    crate::register_runtime_fn(env, "kv/close", |args| {
        check_arity!(args, "kv/close", 1);
        let name = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let gate = KV_GATES.with(|g| g.borrow().get(&name).cloned());
        if KV_STORES.with(|s| matches!(s.borrow().get(&name), Some(KvSlot::CheckedOut))) {
            return Err(busy_err("kv/close", &name));
        }
        prepare_terminal_gate(gate.as_ref(), "kv/close")?;
        KV_STORES.with(|s| {
            let mut stores = s.borrow_mut();
            match stores.get(&name) {
                Some(KvSlot::CheckedOut) => unreachable!("busy state checked above"),
                Some(KvSlot::Available(store)) => {
                    let _ = flush_store(store, effective_bounds());
                }
                Some(KvSlot::Tombstone(_)) | None => {}
            }
            stores.remove(&name);
        });
        let remove_name = name;
        finish_terminal_gate(
            gate,
            Rc::new(move |id| {
                KV_GATES.with(|g| {
                    let mut gates = g.borrow_mut();
                    if gates.get(&remove_name).map(ResourceGateHandle::id) == Some(id) {
                        gates.remove(&remove_name);
                    }
                });
            }),
            Ok(Value::nil()),
        )
    });
}

/// Serialize the whole store and write it through. The serialized size is the
/// final byte-cap gate: an over-cap store is refused *before* the write touches
/// disk, so the on-disk file is never replaced with an oversized blob (`kv/set`'s
/// pre-dispatch value/item admission normally rejects first; this is the backstop
/// against accumulation across many in-cap writes).
fn flush_store(store: &KvStore, bounds: KvBounds) -> Result<(), SemaError> {
    let json = serde_json::to_string_pretty(&store.data)
        .map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    if json.len() as u64 > bounds.max_store_bytes {
        return Err(store_bytes_cap_err(
            "kv/flush",
            &format!("store {}", store.path),
            json.len() as u64,
            bounds.max_store_bytes,
        ));
    }
    std::fs::write(&store.path, json).map_err(|e| SemaError::Io(format!("kv/flush: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bounds-presence guard: the shipped ceilings are finite and nonzero, and an
    /// override can only *lower* them — never raise a store past the hard cap.
    #[test]
    fn runtime_bounds_are_finite_and_clamp_overrides() {
        // The shipped ceilings are the module consts (finite, nonzero literals).
        assert_eq!(KV_RUNTIME_BOUNDS.max_store_bytes, KV_MAX_STORE_BYTES);
        assert_eq!(KV_RUNTIME_BOUNDS.max_items, KV_MAX_ITEMS);

        set_kv_bounds_override(Some((u64::MAX, usize::MAX)));
        let raised = effective_bounds();
        assert_eq!(raised.max_store_bytes, KV_MAX_STORE_BYTES, "override cannot raise the byte ceiling");
        assert_eq!(raised.max_items, KV_MAX_ITEMS, "override cannot raise the item ceiling");

        set_kv_bounds_override(Some((16, 2)));
        let lowered = effective_bounds();
        assert_eq!(lowered.max_store_bytes, 16);
        assert_eq!(lowered.max_items, 2);
        set_kv_bounds_override(None);
        assert_eq!(effective_bounds().max_store_bytes, KV_MAX_STORE_BYTES);
    }

    /// An oversized backing file is rejected both by the metadata preflight and by
    /// the capped read, and neither allocates more than the cap.
    #[test]
    fn oversized_store_load_rejected_by_metadata_and_capped_read() {
        let path = std::env::temp_dir().join(format!(
            "sema-kv-bounds-load-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, vec![b'x'; 4096]).expect("seed oversized file");
        let p = path.to_string_lossy().to_string();
        let bounds = KvBounds {
            max_store_bytes: 1024,
            max_items: 10,
        };

        let pre = preflight_store_size("kv/open", &p, bounds)
            .expect_err("metadata preflight must reject the oversized store");
        assert!(pre.to_string().contains("kv store limit"), "{pre}");

        let read = read_or_init_store(&p, bounds)
            .expect_err("capped read must reject the oversized store");
        assert!(read.to_string().contains("kv store limit"), "{read}");

        // A missing file is admitted as an empty store (fresh open).
        let _ = std::fs::remove_file(&path);
        assert!(preflight_store_size("kv/open", &p, bounds).is_ok());
        assert!(read_or_init_store(&p, bounds)
            .expect("missing file → empty store")
            .is_empty());
    }

    /// `check_value_bytes` rejects an over-cap value with no store access;
    /// `check_item_cap` rejects a *new* key past the item cap on an owned store
    /// but admits overwriting an existing key even at the cap — both before any
    /// mutation.
    #[test]
    fn set_admission_rejects_over_cap_value_and_item_count() {
        let name = "bounds-guard-set";
        let bounds = KvBounds {
            max_store_bytes: 16,
            max_items: 2,
        };

        // Value-size admission is store-free.
        let big = serde_json::Value::String("x".repeat(64));
        let over_value = check_value_bytes("kv/set", name, "k", &big, bounds)
            .expect_err("a value over the byte cap must be rejected");
        assert!(over_value.to_string().contains("kv store limit"), "{over_value}");
        let small = serde_json::Value::from(1);
        check_value_bytes("kv/set", name, "k", &small, bounds)
            .expect("an in-cap value is admitted");

        // Item-count admission runs on an owned store.
        let mut store = KvStore {
            path: "/definitely/not/written".to_string(),
            data: serde_json::Map::new(),
        };
        store.data.insert("a".into(), serde_json::Value::from(1));
        store.data.insert("b".into(), serde_json::Value::from(2));

        let over_items = check_item_cap("kv/set", name, &store, "c", bounds)
            .expect_err("a new key past the item cap must be rejected");
        assert!(over_items.to_string().contains("item"), "{over_items}");

        // Overwriting an existing key is admitted even at the item cap.
        check_item_cap("kv/set", name, &store, "a", bounds)
            .expect("overwriting an existing key at the item cap is admitted");
    }
}
