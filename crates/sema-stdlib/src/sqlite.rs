//! SQLite database primitives (`db/*`).
//!
//! Connections live in a thread-local registry keyed by handle (the path, or
//! an explicit name for `db/open`/`db/open-memory`). `rusqlite::Connection`
//! is `Send` (asserted below), so a call that blocks on real I/O — opening
//! the file, or running a statement against it — can move the connection
//! onto the I/O pool's blocking tier and back instead of blocking the VM
//! thread for the operation's whole duration. Inside an `async/spawn`'d task
//! that would otherwise stall every sibling on the cooperative scheduler.
//!
//! `db/open`/`db/open-memory` offload the open itself via `fs_offload`
//! (io.rs): there is no existing connection to contend over, so the poller
//! simply inserts the freshly-opened `Connection` into the registry on
//! completion — mirroring `file/read`'s shape exactly. Every opened connection
//! carries a bounded `busy_timeout` (so a checkout op blocked on a locked
//! database yields `SQLITE_BUSY` in bounded time rather than pinning a worker
//! forever) and has its `Send` [`rusqlite::InterruptHandle`] captured beside
//! the registry (`DB_INTERRUPTS`) for the cancellation path below.
//!
//! `db/exec`/`db/exec-batch`/`db/query`/`db/query-one`/`db/tables` run
//! against an EXISTING connection, so they use the CHECKOUT pattern (see
//! `proc.rs`'s module doc comment for the canonical writeup this mirrors): the
//! registry slot is `Available(Connection)` / `CheckedOut` / `Tombstone(msg)`.
//! The offload takes the `Connection` out of the slot for its duration; any
//! other `db/*` op on the SAME handle sees `CheckedOut` and either errors
//! clearly (the sync path, and non-offloaded ops like `db/last-insert-id`) or
//! queues (an async caller's `IoHandle` re-attempts the checkout every poll —
//! the `Acquire` phase — until the slot frees up, then runs its own offload).
//! The offload's poller reinstalls the `Connection` as `Available` and calls
//! `notify_io_complete()` so a sibling queued on the same handle can't miss
//! the wakeup. Row → `Value` conversion (`rows_to_value`/`row_to_value`)
//! happens in the poller on the VM thread from an owned `Send` intermediate
//! (`Vec<(String, SqlValue)>` per row) — `rusqlite::types::Value` is already
//! `Send`, so it crosses the boundary directly with no extra wrapper type.
//!
//! ## Interrupt-then-reclaim cancellation (`INTERRUPTIBLE`)
//!
//! A checkout op is offloaded through [`checkout_runtime`], which hands the
//! checkout `CheckoutOp` a real `abort` (fire the connection's interrupt
//! handle + flag the op interrupted) and a `reclaim` closure. On `async/cancel`
//! the runtime fires the abort: the blocked statement returns `SQLITE_INTERRUPT`
//! promptly, the worker op rolls back an open transaction
//! (`!conn.is_autocommit()`), and the connection is handed back through a shared
//! cell. The reclaim step then reinstalls the connection `Available` (never
//! tombstones it) so the handle stays usable, and the closed gate lets a fresh
//! op re-create it. Late results are rejected — the cancelled task settled the
//! moment the abort fired. Bounded result caps (`DB_MAX_RESULT_ROWS`/
//! `DB_MAX_RESULT_BYTES`, with an optional lower per-call override) are resolved
//! pre-dispatch and enforced incrementally inside `collect_query_rows`/
//! `collect_tables`, so an oversized result is rejected at the boundary without
//! ever buffering the whole set.
//!
//! `db/last-insert-id` reads `Connection::last_insert_rowid()` — an in-memory
//! field on the handle, no I/O — so it stays fully synchronous even inside
//! async context, but is still checkout-aware (`with_conn`) so it reports a
//! clear busy error instead of "no such handle" when a concurrent op holds
//! the connection checked out.
//!
//! At top level (no scheduler) every builtin keeps today's synchronous shape
//! byte-for-byte.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::time::Duration;

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection, InterruptHandle};
use sema_core::runtime::{CompletionKind, NativeOutcome, NativeResult, ResourceGateHandle};
use sema_core::{check_arity, in_runtime_quantum, SemaError, Value};

use crate::runtime_offload::{
    checkout_external, finish_terminal_gate, prepare_terminal_gate, CheckoutOp,
};

/// Completion-kind tag for `db/*` external waits ("db\0\0").
const DB_COMPLETION_KIND: u64 = 0x6462_0000;

/// Busy timeout every opened connection carries: a checkout op blocked on a
/// locked database yields `SQLITE_BUSY` after this bound instead of parking a
/// blocking worker forever. Distinct from rusqlite's own default so a
/// `PRAGMA busy_timeout` read proves `db/open` set it.
const DB_BUSY_TIMEOUT_MS: u64 = 10_000;

/// Hard ceiling on the rows a single `db/query`/`db/tables` result may buffer
/// before it is rejected. The result is eagerly materialized into a Sema
/// list-of-maps on the VM thread, so an unbounded query would exhaust memory;
/// this cap makes an oversized result a clean error, not an OOM.
const DB_MAX_RESULT_ROWS: u64 = 1_000_000;
/// Hard ceiling on the bytes a single result may buffer (summed cell sizes).
const DB_MAX_RESULT_BYTES: u64 = 512 * 1024 * 1024;

// `Connection` moves across the offload boundary (open + every checkout op).
// This compiles only if it stays `Send`; a future rusqlite upgrade that
// breaks it fails here, not with an opaque trait-bound error deep in
// `sema_io::io_spawn_blocking`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Connection>();
    // The interrupt handle crosses to the abort hook and must stay `Send`.
    assert_send::<InterruptHandle>();
};

/// A registry slot. `CheckedOut` is the moment between an offload taking the
/// `Connection` out and the poller (or the interrupt-reclaim path) reinstalling
/// it; every other `db/*` op treats it as "busy, try again once the in-flight
/// op resolves". `Tombstone` is terminal: set only when an offloaded op's worker
/// vanishes unexpectedly (panic / lost completion). A cancelled interrupt does
/// NOT tombstone — it reclaims the connection and reinstalls it `Available` (see
/// the module doc comment); `db/close` frees a tombstoned slot.
enum DbSlot {
    Available(Connection),
    CheckedOut,
    Tombstone(String),
}

thread_local! {
    static DB_CONNECTIONS: RefCell<HashMap<String, DbSlot>> = RefCell::new(HashMap::new());
    /// Per-handle owning resource-gate capability, created lazily on the first
    /// offloaded op and reused for later ops (dropped on `db/close`). The gate
    /// provides FIFO mutual exclusion for the checkout slot.
    static DB_GATES: RefCell<HashMap<String, ResourceGateHandle>> = RefCell::new(HashMap::new());
    /// Per-handle `Send` interrupt handle captured at open, kept alongside the
    /// slot so a checkout op's `abort` can interrupt the connection even while
    /// it is `CheckedOut`. Dropped on `db/close`.
    static DB_INTERRUPTS: RefCell<HashMap<String, Arc<InterruptHandle>>> =
        RefCell::new(HashMap::new());
    /// Optional per-call result-cap override (lowered, never raised above the
    /// hard ceilings). `None` uses the module ceilings.
    static DB_RESULT_CAPS_OVERRIDE: Cell<Option<DbResultCaps>> = const { Cell::new(None) };
    /// Whether this thread's interpreter has an interpreter-teardown hook wired
    /// for the db registry (C6) — see `proc.rs`'s `PROC_TEARDOWN_REGISTERED`.
    static DB_TEARDOWN_REGISTERED: Cell<bool> = const { Cell::new(false) };
}

/// Register the interpreter-teardown hook for the db registry against `ctx`
/// exactly once per interpreter (C6). Called at `db/open`/`db/open-memory` so a
/// connection opened but never `db/close`d is still closed on interpreter drop.
fn ensure_teardown_hook(ctx: &sema_core::EvalContext) {
    if !DB_TEARDOWN_REGISTERED.with(|c| c.replace(true)) {
        ctx.register_interpreter_teardown_hook(teardown_connections);
    }
}

/// Interpreter-drop teardown for the db registry (C6). Dropping every slot
/// closes the underlying connection (an `Available` connection's OS handle; a
/// `CheckedOut` connection is held by an offloaded worker whose statement was
/// interrupted on cancel — R16A — so the slot is simply dropped). The captured
/// interrupt handles are dropped too, and gates are closed so any parked waiter
/// fails fast.
fn teardown_connections() {
    DB_CONNECTIONS.with(|c| c.borrow_mut().clear());
    DB_INTERRUPTS.with(|m| m.borrow_mut().clear());
    DB_GATES.with(|g| {
        for (_, gate) in g.borrow_mut().drain() {
            let _ = gate.close();
        }
    });
    DB_TEARDOWN_REGISTERED.with(|c| c.set(false));
}

/// Bounded result caps resolved pre-dispatch and enforced incrementally by the
/// row collectors.
#[derive(Clone, Copy)]
struct DbResultCaps {
    max_rows: u64,
    max_bytes: u64,
}

/// The effective result caps for the current call: the module hard ceilings,
/// lowered by any per-call override (never raised above the ceilings). Read on
/// the VM thread pre-dispatch, then captured by the offloaded op.
fn effective_result_caps() -> DbResultCaps {
    let ceiling = DbResultCaps {
        max_rows: DB_MAX_RESULT_ROWS,
        max_bytes: DB_MAX_RESULT_BYTES,
    };
    DB_RESULT_CAPS_OVERRIDE
        .with(Cell::get)
        .map_or(ceiling, |over| DbResultCaps {
            max_rows: over.max_rows.min(ceiling.max_rows),
            max_bytes: over.max_bytes.min(ceiling.max_bytes),
        })
}

/// Lower the per-call result caps (clamped to the hard ceilings) for subsequent
/// `db/query`/`db/tables` calls on this thread, or clear the override with
/// `None`. The hard ceilings and the interrupt/reclaim path are unaffected;
/// this is the seam a bounded-result caller (and the regression suite) drives.
pub fn set_db_result_caps_override(caps: Option<(u64, u64)>) {
    DB_RESULT_CAPS_OVERRIDE.with(|cell| {
        cell.set(caps.map(|(max_rows, max_bytes)| DbResultCaps { max_rows, max_bytes }));
    });
}

/// A checkout op failure: a rusqlite error (rendered `op: {e}`, and possibly
/// triggering an interrupt rollback), or a pre-buffered result-cap rejection
/// (its message already carries the `op:` prefix).
#[derive(Debug)]
enum DbOpError {
    Sqlite(rusqlite::Error),
    Cap(String),
}

fn db_op_err_to_sema(op: &str, error: DbOpError) -> SemaError {
    match error {
        DbOpError::Sqlite(error) => SemaError::eval(format!("{op}: {error}")),
        DbOpError::Cap(message) => SemaError::eval(message),
    }
}

fn sql_value_bytes(value: &SqlValue) -> u64 {
    match value {
        SqlValue::Null => 1,
        SqlValue::Integer(_) | SqlValue::Real(_) => 8,
        SqlValue::Text(text) => text.len() as u64,
        SqlValue::Blob(bytes) => bytes.len() as u64,
    }
}

fn row_bytes(row: &[(String, SqlValue)]) -> u64 {
    row.iter()
        .map(|(name, value)| name.len() as u64 + sql_value_bytes(value))
        .sum()
}

fn sema_to_sql(v: &Value) -> SqlValue {
    if v.is_nil() {
        SqlValue::Null
    } else if let Some(b) = v.as_bool() {
        SqlValue::Integer(b as i64)
    } else if let Some(i) = v.as_int() {
        SqlValue::Integer(i)
    } else if let Some(f) = v.as_float() {
        SqlValue::Real(f)
    } else if let Some(s) = v.as_str() {
        SqlValue::Text(s.to_string())
    } else if let Some(bytes) = v.as_bytevector() {
        SqlValue::Blob(bytes.to_vec())
    } else {
        SqlValue::Text(v.to_string())
    }
}

fn sql_to_sema(v: &SqlValue) -> Value {
    match v {
        SqlValue::Null => Value::nil(),
        SqlValue::Integer(i) => Value::int(*i),
        SqlValue::Real(f) => Value::float(*f),
        SqlValue::Text(s) => Value::string(s),
        SqlValue::Blob(b) => Value::bytevector(b.clone()),
    }
}

fn missing_err(op: &str, handle: &str) -> SemaError {
    SemaError::eval(format!("{op}: no open database '{handle}'"))
}

/// `op` was attempted while an offload had `handle` checked out.
fn busy_err(op: &str, handle: &str) -> SemaError {
    SemaError::eval(format!(
        "{op}: database '{handle}' is busy — another db/* call is in flight on it"
    ))
    .with_hint("wait for the in-flight db/* call on this handle to resolve before calling another")
}

/// `op` was attempted on a handle whose in-flight offload was cancelled.
fn tombstone_err(op: &str, handle: &str, reason: &str) -> SemaError {
    SemaError::eval(format!(
        "{op}: database '{handle}' is no longer usable: {reason}"
    ))
}

/// Pre-render `op: {e}` through the same `SemaError::eval` constructor the
/// sync path raises, so the message text an async rejection carries is
/// substring-identical to what the sync path would display for the same
/// failure (mirrors `fs_io_msg` in io.rs, matched to `eval` instead of `Io`
/// since every sync `db/*` error already goes through `SemaError::eval`).
fn eval_msg(op: &str, e: impl std::fmt::Display) -> String {
    SemaError::eval(format!("{op}: {e}")).to_string()
}

/// Finish opening a connection: apply the bounded busy timeout, capture its
/// (`Send`) interrupt handle for the checkout abort path, and install it
/// `Available` under `key`. Shared by `db/open`/`db/open-memory` (sync path and
/// async decode).
fn finish_open(op: &'static str, key: String, conn: Connection) -> Result<Value, SemaError> {
    conn.busy_timeout(Duration::from_millis(DB_BUSY_TIMEOUT_MS))
        .map_err(|e| SemaError::eval(format!("{op}: {e}")))?;
    let interrupt = Arc::new(conn.get_interrupt_handle());
    DB_INTERRUPTS.with(|m| {
        m.borrow_mut().insert(key.clone(), interrupt);
    });
    DB_CONNECTIONS.with(|c| {
        c.borrow_mut().insert(key.clone(), DbSlot::Available(conn));
    });
    Ok(Value::string(&key))
}

/// Build the checkout abort hook: flag the op interrupted (so the worker rolls
/// back an open transaction before releasing the connection) and fire the
/// connection's SQLite interrupt handle so a blocked statement returns
/// `SQLITE_INTERRUPT` promptly. Always returns a closure, so every sqlite
/// `CheckoutOp` carries a real `abort: Some(_)`.
fn checkout_interrupt_abort(handle: String, interrupted: Arc<AtomicBool>) -> Box<dyn FnOnce()> {
    Box::new(move || {
        interrupted.store(true, Ordering::SeqCst);
        DB_INTERRUPTS.with(|m| {
            if let Some(interrupt) = m.borrow().get(&handle) {
                interrupt.interrupt();
            }
        });
    })
}

/// Sync-path / non-offloaded accessor: look up `handle` for an op that only
/// needs `&Connection`, translating the other slot states into a clear,
/// `op`-specific error instead of ever panicking on the enum shape. Used both
/// by ops that never offload (`db/last-insert-id`) and by every offloadable
/// op's OWN top-level (non-async) branch.
fn with_conn<R>(
    op: &str,
    handle: &str,
    f: impl FnOnce(&Connection) -> Result<R, SemaError>,
) -> Result<R, SemaError> {
    DB_CONNECTIONS.with(|c| {
        let conns = c.borrow();
        match conns.get(handle) {
            Some(DbSlot::Available(conn)) => f(conn),
            Some(DbSlot::CheckedOut) => Err(busy_err(op, handle)),
            Some(DbSlot::Tombstone(msg)) => Err(tombstone_err(op, handle, msg)),
            None => Err(missing_err(op, handle)),
        }
    })
}

/// Take `handle`'s connection out of its slot once its gate is owned. A
/// tombstoned/missing slot (a prior op cancelled mid-flight) fails clearly.
fn take_conn(op_name: &'static str, handle: &str) -> Result<Connection, SemaError> {
    DB_CONNECTIONS.with(|c| {
        let mut conns = c.borrow_mut();
        match conns.get_mut(handle) {
            Some(slot @ DbSlot::Available(_)) => {
                let DbSlot::Available(conn) = std::mem::replace(slot, DbSlot::CheckedOut) else {
                    unreachable!("just matched Available")
                };
                Ok(conn)
            }
            Some(DbSlot::CheckedOut) => Err(busy_err(op_name, handle)),
            Some(DbSlot::Tombstone(msg)) => Err(tombstone_err(op_name, handle, msg)),
            None => Err(missing_err(op_name, handle)),
        }
    })
}

/// Offload one blocking rusqlite operation on the connection named `handle`
/// through the CHECKOUT pattern (see the module doc comment) under the unified
/// runtime: acquire the handle's [`ResourceGate`] (creating it on first use),
/// take the `Connection` out of the slot, run `op` off the VM thread on the
/// executor's blocking tier, reinstall the `Connection` and decode the result on
/// the VM thread, then release the gate. `op` runs on the blocking worker;
/// `decode` builds the final `Value`.
///
/// Cancellation is INTERRUPTIBLE: the abort fires the connection's interrupt
/// handle so a blocked statement returns `SQLITE_INTERRUPT` promptly; the worker
/// op then rolls back an open transaction and hands the connection back through
/// a shared cell, and the reclaim step reinstalls it `Available` (the slot is
/// NOT tombstoned) so the handle stays usable while the closed gate lets a fresh
/// op re-create it. Only a worker LOSS (panic / lost completion) tombstones.
fn checkout_runtime<T: Send + 'static>(
    op_name: &'static str,
    handle: String,
    op: impl FnOnce(&Connection) -> Result<T, DbOpError> + Send + 'static,
    decode: impl FnOnce(T) -> Value + 'static,
) -> NativeResult {
    let kind =
        CompletionKind::try_from_raw(DB_COMPLETION_KIND).expect("db completion kind is nonzero");
    let gate = DB_GATES.with(|g| g.borrow().get(&handle).cloned());

    // Shared connection cell: a mid-op cancel interrupts the worker, then the
    // reclaim step retrieves the connection from here and reinstalls it
    // `Available` — instead of tombstoning the slot.
    let shared: Arc<Mutex<Option<Connection>>> = Arc::new(Mutex::new(None));
    let shared_take = Arc::clone(&shared);
    let shared_reclaim = Arc::clone(&shared);

    // Set by the abort on cancellation so the worker op rolls back an open
    // transaction before it releases the reclaimed connection.
    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupted_op = Arc::clone(&interrupted);

    let h_take = handle.clone();
    let h_reinstall = handle.clone();
    let h_reclaim = handle.clone();
    let h_tomb = handle.clone();
    let h_remove = handle.clone();
    let h_abort = handle.clone();
    let h_store = handle;

    checkout_external(CheckoutOp {
        op_name,
        kind,
        gate,
        store_gate: Box::new(move |id| {
            DB_GATES.with(|g| {
                g.borrow_mut().insert(h_store, id);
            });
        }),
        remove_gate: Rc::new(move |id| {
            DB_GATES.with(|g| {
                let mut gates = g.borrow_mut();
                if gates.get(&h_remove).map(ResourceGateHandle::id) == Some(id) {
                    gates.remove(&h_remove);
                }
            });
        }),
        take: Box::new(move || {
            let conn = take_conn(op_name, &h_take)?;
            *shared_take.lock().unwrap_or_else(PoisonError::into_inner) = Some(conn);
            Ok(shared_take)
        }),
        op: Box::new(
            move |res: &mut Arc<Mutex<Option<Connection>>>| -> Result<T, String> {
                let guard = res.lock().unwrap_or_else(PoisonError::into_inner);
                let Some(conn) = guard.as_ref() else {
                    return Err(format!(
                        "{op_name}: connection was reclaimed after cancellation"
                    ));
                };
                match op(conn) {
                    Ok(value) => Ok(value),
                    Err(DbOpError::Sqlite(error)) => {
                        // A cancelled op is interrupted mid-statement; roll back
                        // any open transaction so the reclaimed connection is
                        // clean for the next caller.
                        if interrupted_op.load(Ordering::SeqCst) && !conn.is_autocommit() {
                            let _ = conn.execute_batch("ROLLBACK");
                        }
                        Err(eval_msg(op_name, error))
                    }
                    Err(DbOpError::Cap(message)) => Err(message),
                }
            },
        ),
        reinstall: Box::new(move |res: Arc<Mutex<Option<Connection>>>| {
            if let Some(conn) = res.lock().unwrap_or_else(PoisonError::into_inner).take() {
                DB_CONNECTIONS.with(|c| {
                    c.borrow_mut().insert(h_reinstall, DbSlot::Available(conn));
                });
            }
        }),
        decode: Box::new(move |t| Ok(decode(t))),
        success_value: None,
        tombstone: Rc::new(move |msg| {
            DB_CONNECTIONS.with(|c| {
                c.borrow_mut()
                    .insert(h_tomb.clone(), DbSlot::Tombstone(msg));
            });
        }),
        abort: Some(checkout_interrupt_abort(h_abort, interrupted)),
        reclaim: Some(Box::new(move || -> bool {
            // On cancel the interrupted worker op is either still running (lock
            // held → retry next reap) or has returned the connection into the
            // shared cell (take it and reinstall Available). Either way the slot
            // ends usable rather than tombstoned.
            match shared_reclaim.try_lock() {
                Ok(mut guard) => {
                    if let Some(conn) = guard.take() {
                        DB_CONNECTIONS.with(|c| {
                            c.borrow_mut()
                                .insert(h_reclaim.clone(), DbSlot::Available(conn));
                        });
                    }
                    true
                }
                Err(TryLockError::WouldBlock) => false,
                Err(TryLockError::Poisoned(poison)) => {
                    if let Some(conn) = poison.into_inner().take() {
                        DB_CONNECTIONS.with(|c| {
                            c.borrow_mut()
                                .insert(h_reclaim.clone(), DbSlot::Available(conn));
                        });
                    }
                    true
                }
            }
        })),
        terminal_on_success: false,
    })
}

/// Run `sql` against `conn` with `params`, collecting every row into owned
/// `(column, value)` pairs — the `Send` intermediate that crosses the offload
/// boundary. The result caps are enforced incrementally: the collector rejects
/// at the row/byte boundary without buffering the whole result. `Value`/map
/// construction happens in `rows_to_value`/`row_to_value`, on the VM thread,
/// never inside the offload.
fn collect_query_rows(
    conn: &Connection,
    sql: &str,
    params: &[SqlValue],
    caps: DbResultCaps,
) -> Result<Vec<Vec<(String, SqlValue)>>, DbOpError> {
    let mut stmt = conn.prepare(sql).map_err(DbOpError::Sqlite)?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap().to_string())
        .collect();
    let mut rows = stmt
        .query_map(params_from_iter(params.iter()), |row| {
            let mut r = Vec::with_capacity(col_count);
            for (i, name) in col_names.iter().enumerate() {
                let val: SqlValue = row.get(i)?;
                r.push((name.clone(), val));
            }
            Ok(r)
        })
        .map_err(DbOpError::Sqlite)?;
    let mut out: Vec<Vec<(String, SqlValue)>> = Vec::new();
    let mut total_bytes: u64 = 0;
    while let Some(row) = rows.next().transpose().map_err(DbOpError::Sqlite)? {
        if out.len() as u64 >= caps.max_rows {
            return Err(DbOpError::Cap(format!(
                "db/query: result exceeds the maximum of {} rows",
                caps.max_rows
            )));
        }
        total_bytes = total_bytes.saturating_add(row_bytes(&row));
        if total_bytes > caps.max_bytes {
            return Err(DbOpError::Cap(format!(
                "db/query: result exceeds the maximum of {} bytes",
                caps.max_bytes
            )));
        }
        out.push(row);
    }
    Ok(out)
}

/// Run `sql` against `conn` with `params`, stepping the cursor exactly once —
/// the `db/query-one` counterpart to `collect_query_rows`. Mirrors the
/// pre-offload sync implementation's `stmt.query_map(...).next()`: later rows
/// (and whatever runtime error they might raise, e.g. SQLite's "integer
/// overflow") are never evaluated, so a query whose first row is fine but a
/// later row errors still succeeds, and a huge result set costs O(1) rather
/// than buffering every row before returning the first.
fn collect_first_query_row(
    conn: &Connection,
    sql: &str,
    params: &[SqlValue],
) -> Result<Option<Vec<(String, SqlValue)>>, DbOpError> {
    let mut stmt = conn.prepare(sql).map_err(DbOpError::Sqlite)?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap().to_string())
        .collect();
    let mut rows = stmt
        .query_map(params_from_iter(params.iter()), |row| {
            let mut r = Vec::with_capacity(col_count);
            for (i, name) in col_names.iter().enumerate() {
                let val: SqlValue = row.get(i)?;
                r.push((name.clone(), val));
            }
            Ok(r)
        })
        .map_err(DbOpError::Sqlite)?;
    rows.next().transpose().map_err(DbOpError::Sqlite)
}

fn row_pairs_to_map(row: Vec<(String, SqlValue)>) -> BTreeMap<Value, Value> {
    let mut map = BTreeMap::new();
    for (name, val) in row {
        map.insert(Value::keyword(&name), sql_to_sema(&val));
    }
    map
}

fn rows_to_value(rows: Vec<Vec<(String, SqlValue)>>) -> Value {
    Value::list(
        rows.into_iter()
            .map(|row| Value::map(row_pairs_to_map(row)))
            .collect(),
    )
}

fn row_to_value(row: Option<Vec<(String, SqlValue)>>) -> Value {
    match row {
        Some(row) => Value::map(row_pairs_to_map(row)),
        None => Value::nil(),
    }
}

fn collect_tables(conn: &Connection, caps: DbResultCaps) -> Result<Vec<String>, DbOpError> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(DbOpError::Sqlite)?;
    let mut rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(DbOpError::Sqlite)?;
    let mut names: Vec<String> = Vec::new();
    let mut total_bytes: u64 = 0;
    while let Some(name) = rows.next().transpose().map_err(DbOpError::Sqlite)? {
        if names.len() as u64 >= caps.max_rows {
            return Err(DbOpError::Cap(format!(
                "db/tables: result exceeds the maximum of {} rows",
                caps.max_rows
            )));
        }
        total_bytes = total_bytes.saturating_add(name.len() as u64);
        if total_bytes > caps.max_bytes {
            return Err(DbOpError::Cap(format!(
                "db/tables: result exceeds the maximum of {} bytes",
                caps.max_bytes
            )));
        }
        names.push(name);
    }
    Ok(names)
}

fn tables_to_value(names: Vec<String>) -> Value {
    Value::list(names.iter().map(|s| Value::string(s)).collect())
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (db/open path) or (db/open name path)
    crate::register_runtime_fn_path_gated_ctx(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/open",
        &[0],
        |ctx, args| {
            // A connection is about to enter the registry: wire the
            // interpreter-teardown hook (idempotent) on the VM thread before the
            // open dispatches — the async decoder (`finish_open`) has no `ctx`.
            ensure_teardown_hook(ctx);
            let (key, path) = match args.len() {
                1 => {
                    let path = args[0]
                        .as_str()
                        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
                    (path.to_string(), path.to_string())
                }
                2 => {
                    let name = args[0]
                        .as_str()
                        .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
                    let path = args[1]
                        .as_str()
                        .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
                    (name.to_string(), path.to_string())
                }
                _ => return Err(SemaError::arity("db/open", "1 or 2", args.len())),
            };

            if in_runtime_quantum() {
                let kind = CompletionKind::try_from_raw(DB_COMPLETION_KIND)
                    .expect("db completion kind is nonzero");
                let key_for_decode = key;
                return crate::runtime_offload::external_io_interruptible_try(
                    "db/open",
                    kind,
                    "db/open",
                    move |conn: Connection| finish_open("db/open", key_for_decode, conn),
                    move || async move {
                        let conn = Connection::open(&path).map_err(|e| eval_msg("db/open", e))?;
                        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                            .map_err(|e| eval_msg("db/open", e))?;
                        Ok(conn)
                    },
                );
            }

            let conn =
                Connection::open(&path).map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
            finish_open("db/open", key, conn).map(NativeOutcome::Return)
        },
    );

    // (db/open-memory) or (db/open-memory name)
    crate::register_runtime_fn_path_gated_ctx(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/open-memory",
        &[],
        |ctx, args| {
            ensure_teardown_hook(ctx);
            let name = if args.is_empty() {
                ":memory:".to_string()
            } else if args.len() == 1 {
                args[0]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                    .to_string()
            } else {
                return Err(SemaError::arity("db/open-memory", "0 or 1", args.len()));
            };

            if in_runtime_quantum() {
                let kind = CompletionKind::try_from_raw(DB_COMPLETION_KIND)
                    .expect("db completion kind is nonzero");
                let name_for_decode = name;
                return crate::runtime_offload::external_io_interruptible_try(
                    "db/open-memory",
                    kind,
                    "db/open-memory",
                    move |conn: Connection| finish_open("db/open-memory", name_for_decode, conn),
                    move || async move {
                        let conn = Connection::open_in_memory()
                            .map_err(|e| eval_msg("db/open-memory", e))?;
                        conn.execute_batch("PRAGMA foreign_keys=ON;")
                            .map_err(|e| eval_msg("db/open-memory", e))?;
                        Ok(conn)
                    },
                );
            }

            let conn = Connection::open_in_memory()
                .map_err(|e| SemaError::eval(format!("db/open-memory: {e}")))?;
            conn.execute_batch("PRAGMA foreign_keys=ON;")
                .map_err(|e| SemaError::eval(format!("db/open-memory: {e}")))?;
            finish_open("db/open-memory", name, conn).map(NativeOutcome::Return)
        },
    );

    // (db/exec handle sql ...params) -> int (affected rows)
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/exec",
        &[],
        |args| {
            if args.len() < 2 {
                return Err(SemaError::arity("db/exec", "2+", args.len()));
            }
            let handle = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            let sql = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                .to_string();
            let params: Vec<SqlValue> = args[2..].iter().map(sema_to_sql).collect();

            if in_runtime_quantum() {
                return checkout_runtime(
                    "db/exec",
                    handle,
                    move |conn| {
                        conn.execute(&sql, params_from_iter(params.iter()))
                            .map(|n| n as i64)
                            .map_err(DbOpError::Sqlite)
                    },
                    Value::int,
                );
            }

            with_conn("db/exec", &handle, |conn| {
                conn.execute(&sql, params_from_iter(params.iter()))
                    .map(|n| Value::int(n as i64))
                    .map_err(|e| SemaError::eval(format!("db/exec: {e}")))
            })
            .map(NativeOutcome::Return)
        },
    );

    // (db/exec-batch handle sql) -> nil (execute multiple statements)
    // STATIC SQL ONLY: no parameter binding — the string is run verbatim.
    // Never interpolate user-controlled input; use parameterized db/exec for that.
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/exec-batch",
        &[],
        |args| {
            check_arity!(args, "db/exec-batch", 2);
            let handle = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            let sql = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                .to_string();

            if in_runtime_quantum() {
                return checkout_runtime(
                    "db/exec-batch",
                    handle,
                    move |conn| conn.execute_batch(&sql).map_err(DbOpError::Sqlite),
                    |()| Value::nil(),
                );
            }

            with_conn("db/exec-batch", &handle, |conn| {
                conn.execute_batch(&sql)
                    .map(|()| Value::nil())
                    .map_err(|e| SemaError::eval(format!("db/exec-batch: {e}")))
            })
            .map(NativeOutcome::Return)
        },
    );

    // (db/query handle sql ...params) -> list of maps
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_READ,
        "db/query",
        &[],
        |args| {
            if args.len() < 2 {
                return Err(SemaError::arity("db/query", "2+", args.len()));
            }
            let handle = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            let sql = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                .to_string();
            let params: Vec<SqlValue> = args[2..].iter().map(sema_to_sql).collect();
            // Resolve the result caps pre-dispatch on the VM thread; the
            // offloaded op captures the (Copy) caps and enforces them.
            let caps = effective_result_caps();

            if in_runtime_quantum() {
                return checkout_runtime(
                    "db/query",
                    handle,
                    move |conn| collect_query_rows(conn, &sql, &params, caps),
                    rows_to_value,
                );
            }

            with_conn("db/query", &handle, |conn| {
                collect_query_rows(conn, &sql, &params, caps)
                    .map(rows_to_value)
                    .map_err(|e| db_op_err_to_sema("db/query", e))
            })
            .map(NativeOutcome::Return)
        },
    );

    // (db/query-one handle sql ...params) -> map or nil
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_READ,
        "db/query-one",
        &[],
        |args| {
            if args.len() < 2 {
                return Err(SemaError::arity("db/query-one", "2+", args.len()));
            }
            let handle = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            let sql = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?
                .to_string();
            let params: Vec<SqlValue> = args[2..].iter().map(sema_to_sql).collect();

            if in_runtime_quantum() {
                return checkout_runtime(
                    "db/query-one",
                    handle,
                    move |conn| collect_first_query_row(conn, &sql, &params),
                    row_to_value,
                );
            }

            with_conn("db/query-one", &handle, |conn| {
                collect_first_query_row(conn, &sql, &params)
                    .map(row_to_value)
                    .map_err(|e| db_op_err_to_sema("db/query-one", e))
            })
            .map(NativeOutcome::Return)
        },
    );

    // (db/last-insert-id handle) -> int
    //
    // Pure in-memory read on the connection handle — no I/O — so it never
    // offloads, but stays checkout-aware (`with_conn`) so a handle busy with
    // an in-flight `db/exec`/`db/query` offload reports BUSY rather than the
    // registry entry appearing to vanish.
    crate::register_fn_gated(
        env,
        sandbox,
        sema_core::Caps::FS_READ,
        "db/last-insert-id",
        |args| {
            check_arity!(args, "db/last-insert-id", 1);
            let handle = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            with_conn("db/last-insert-id", handle, |conn| {
                Ok(Value::int(conn.last_insert_rowid()))
            })
        },
    );

    // (db/tables handle) -> list of strings
    crate::register_runtime_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_READ,
        "db/tables",
        &[],
        |args| {
            check_arity!(args, "db/tables", 1);
            let handle = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();
            let caps = effective_result_caps();

            if in_runtime_quantum() {
                return checkout_runtime(
                    "db/tables",
                    handle,
                    move |conn| collect_tables(conn, caps),
                    tables_to_value,
                );
            }

            with_conn("db/tables", &handle, |conn| {
                collect_tables(conn, caps)
                    .map(tables_to_value)
                    .map_err(|e| db_op_err_to_sema("db/tables", e))
            })
            .map(NativeOutcome::Return)
        },
    );

    // (db/close handle) -> nil
    //
    // A handle checked out by an in-flight offload errors instead of racing
    // the background op for the same `Connection` (matches `proc/close`); a
    // missing or already-tombstoned handle is a silent no-op — `db/close`
    // remains the documented way to free either. The handle's resource gate is
    // dropped here too; when `db/close` succeeds the gate is idle (a busy gate
    // means CheckedOut, which errors above), so no waiter can be stranded.
    crate::register_runtime_fn(env, "db/close", |args| {
        check_arity!(args, "db/close", 1);
        let handle = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        let gate = DB_GATES.with(|g| g.borrow().get(&handle).cloned());
        if DB_CONNECTIONS.with(|c| matches!(c.borrow().get(&handle), Some(DbSlot::CheckedOut))) {
            return Err(busy_err("db/close", &handle));
        }
        prepare_terminal_gate(gate.as_ref(), "db/close")?;
        DB_CONNECTIONS.with(|c| {
            c.borrow_mut().remove(&handle);
        });
        DB_INTERRUPTS.with(|m| {
            m.borrow_mut().remove(&handle);
        });
        let remove_handle = handle;
        finish_terminal_gate(
            gate,
            Rc::new(move |id| {
                DB_GATES.with(|g| {
                    let mut gates = g.borrow_mut();
                    if gates.get(&remove_handle).map(ResourceGateHandle::id) == Some(id) {
                        gates.remove(&remove_handle);
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

    /// Every sqlite `CheckoutOp` is built by `checkout_runtime`, which wraps
    /// `checkout_interrupt_abort` in `abort: Some(_)` unconditionally — so every
    /// exec/exec-batch/query/query-one/tables checkout carries a real interrupt
    /// abort. This proves `db/open` captures the interrupt handle the abort fires
    /// and applies the bounded busy timeout.
    #[test]
    fn open_captures_interrupt_handle_and_sets_busy_timeout() {
        let key = "guard-open-db".to_string();
        let conn = Connection::open_in_memory().expect("open memory db");
        finish_open("db/open-memory", key.clone(), conn).expect("install connection");

        assert!(
            DB_INTERRUPTS.with(|m| m.borrow().contains_key(&key)),
            "open must capture the interrupt handle the checkout abort fires"
        );

        let ms: i64 = DB_CONNECTIONS.with(|c| {
            let conns = c.borrow();
            let DbSlot::Available(conn) = conns.get(&key).expect("open connection") else {
                panic!("connection must be Available after open");
            };
            conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))
                .expect("read busy_timeout")
        });
        assert_eq!(
            ms, DB_BUSY_TIMEOUT_MS as i64,
            "db/open must apply the bounded busy timeout"
        );

        let interrupted = Arc::new(AtomicBool::new(false));
        let abort = checkout_interrupt_abort(key.clone(), Arc::clone(&interrupted));
        abort();
        assert!(
            interrupted.load(Ordering::SeqCst),
            "the checkout abort must flag the op interrupted so it rolls back"
        );

        DB_CONNECTIONS.with(|c| {
            c.borrow_mut().remove(&key);
        });
        DB_INTERRUPTS.with(|m| {
            m.borrow_mut().remove(&key);
        });
    }

    /// Result caps are enforced incrementally: a query one row past the cap is
    /// rejected without buffering the whole set.
    #[test]
    fn result_row_cap_rejects_at_boundary_plus_one() {
        let key = "guard-cap-db".to_string();
        let conn = Connection::open_in_memory().expect("open memory db");
        finish_open("db/open-memory", key.clone(), conn).expect("install connection");

        DB_CONNECTIONS.with(|c| {
            let conns = c.borrow();
            let DbSlot::Available(conn) = conns.get(&key).expect("open connection") else {
                panic!("connection must be Available");
            };
            conn.execute_batch(
                "CREATE TABLE t (v INTEGER);
                 INSERT INTO t (v)
                   WITH RECURSIVE c(x) AS (
                     SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 4
                   )
                   SELECT x FROM c;",
            )
            .expect("seed four rows");

            let caps = DbResultCaps {
                max_rows: 3,
                max_bytes: DB_MAX_RESULT_BYTES,
            };
            let err = collect_query_rows(conn, "SELECT v FROM t ORDER BY v", &[], caps)
                .expect_err("four rows must exceed the three-row cap");
            let DbOpError::Cap(message) = err else {
                panic!("boundary+1 must be a Cap rejection, not a rusqlite error");
            };
            assert!(
                message.contains("exceeds the maximum of 3 rows"),
                "unexpected cap message: {message}"
            );

            // Exactly at the cap succeeds (no rejection buffering the whole set).
            let rows = collect_query_rows(conn, "SELECT v FROM t ORDER BY v LIMIT 3", &[], caps)
                .expect("three rows are within the cap");
            assert_eq!(rows.len(), 3);
        });

        DB_CONNECTIONS.with(|c| {
            c.borrow_mut().remove(&key);
        });
        DB_INTERRUPTS.with(|m| {
            m.borrow_mut().remove(&key);
        });
    }

    /// C6 guard: `ensure_teardown_hook` wires exactly one hook (idempotent), and
    /// firing it closes every open connection (clearing connections, interrupt
    /// handles, and gates) and resets the flag. A real in-memory connection is
    /// used so the drop actually closes a live handle.
    #[test]
    fn teardown_hook_registered_exactly_once_and_closes_connections() {
        let ctx = sema_core::EvalContext::new();
        DB_CONNECTIONS.with(|c| c.borrow_mut().clear());
        DB_INTERRUPTS.with(|m| m.borrow_mut().clear());
        DB_TEARDOWN_REGISTERED.with(|c| c.set(false));

        let key = "guard-teardown-db".to_string();
        let conn = Connection::open_in_memory().expect("open memory db");
        finish_open("db/open-memory", key.clone(), conn).expect("install connection");
        assert!(DB_CONNECTIONS.with(|c| c.borrow().contains_key(&key)));
        assert!(DB_INTERRUPTS.with(|m| m.borrow().contains_key(&key)));

        assert!(!DB_TEARDOWN_REGISTERED.with(Cell::get));
        ensure_teardown_hook(&ctx);
        assert!(
            DB_TEARDOWN_REGISTERED.with(Cell::get),
            "ensure_teardown_hook must register the interpreter hook"
        );
        ensure_teardown_hook(&ctx); // second call is a no-op

        assert!(ctx.try_run_interpreter_teardown_hooks());
        assert!(
            DB_CONNECTIONS.with(|c| c.borrow().is_empty()),
            "teardown must close and drop every connection"
        );
        assert!(
            DB_INTERRUPTS.with(|m| m.borrow().is_empty()),
            "teardown must drop every captured interrupt handle"
        );
        assert!(
            !DB_TEARDOWN_REGISTERED.with(Cell::get),
            "teardown must reset the hook flag so a fresh interpreter re-registers"
        );
    }
}
