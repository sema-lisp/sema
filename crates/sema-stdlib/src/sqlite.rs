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
//! completion — mirroring `file/read`'s shape exactly.
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
//! `db/last-insert-id` reads `Connection::last_insert_rowid()` — an in-memory
//! field on the handle, no I/O — so it stays fully synchronous even inside
//! async context, but is still checkout-aware (`with_conn`) so it reports a
//! clear busy error instead of "no such handle" when a concurrent op holds
//! the connection checked out.
//!
//! At top level (no scheduler) every builtin keeps today's synchronous shape
//! byte-for-byte.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use sema_core::{check_arity, in_async_context, IoHandle, IoPoll, SemaError, Value};

// `Connection` moves across the offload boundary (open + every checkout op).
// This compiles only if it stays `Send`; a future rusqlite upgrade that
// breaks it fails here, not with an opaque trait-bound error deep in
// `sema_io::io_spawn_blocking`.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Connection>();
};

/// A registry slot. `CheckedOut` is the moment between an offload taking the
/// `Connection` out and the poller reinstalling it; every other `db/*` op
/// treats it as "busy, try again once the in-flight op resolves". `Tombstone`
/// is terminal: set only when an offload is cancelled mid-flight (the
/// `Connection` is stuck inside an uncancellable background thread — see
/// `spawn_conn_op`'s doc comment) or its worker vanishes unexpectedly;
/// `db/close` is the only way to free a tombstoned slot.
enum DbSlot {
    Available(Connection),
    CheckedOut,
    Tombstone(String),
}

thread_local! {
    static DB_CONNECTIONS: RefCell<HashMap<String, DbSlot>> = RefCell::new(HashMap::new());
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

/// What crosses the thread boundary from an offloaded connection op back to
/// the poller: the reinstalled `Connection` plus the op's owned `Send`
/// result. Mirrors `proc.rs`'s `WaitOutcome`.
struct ConnOpOutcome<T> {
    conn: Connection,
    result: Result<T, String>,
}

/// The two phases a checkout offload's `IoHandle` cycles through — identical
/// shape to `proc.rs`'s `WaitPhase`. A caller that finds the slot immediately
/// `Available` still starts in `Acquire`; it succeeds on the very first poll
/// and falls through into `Running` in the same tick, so there is exactly one
/// code path for both the uncontended and the queued case.
enum ConnPhase<T> {
    /// Waiting for the slot to become `Available`. Re-checked every poll;
    /// never mutates anything beyond that check, so aborting here is a true
    /// no-op — nothing was ever taken out.
    Acquire,
    /// Holding the checkout; `op` is running on the I/O pool. Resolves with
    /// the reinstalled `Connection` plus the op's result.
    Running(tokio::sync::oneshot::Receiver<ConnOpOutcome<T>>),
}

/// Move `op` on `conn` onto the I/O pool's blocking tier. Cancellation past
/// this point is best-effort by construction (the `Connection` is inside a
/// `spawn_blocking` closure with no abort hook — the same tradeoff every
/// other `spawn_blocking`-based offload in this codebase accepts, see
/// `IoHandle::with_abort`'s doc comment): the caller marks the registry slot
/// `Tombstone` on abort so a later access errors clearly instead of the slot
/// staying `CheckedOut` forever with no one left to reinstall it, but the
/// blocking statement itself keeps running unattended on the worker.
fn spawn_conn_op<T: Send + 'static>(
    mut conn: Connection,
    op: impl FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
) -> tokio::sync::oneshot::Receiver<ConnOpOutcome<T>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    sema_io::io_spawn_blocking(move || {
        let result = op(&mut conn);
        let _ = tx.send(ConnOpOutcome { conn, result });
        // Wake the parked VM thread so it re-polls promptly.
        sema_core::notify_io_complete();
    });
    rx
}

/// Offload one blocking rusqlite operation on the connection named `handle`
/// through the CHECKOUT pattern (see the module doc comment). `op` runs on
/// the I/O pool; `decode` turns its owned `Send` result into the final
/// `Value` on the VM thread when the scheduler polls the completed offload —
/// mirrors `proc.rs`'s `proc_wait_async`/`poll_wait`. Returns `Ok(nil)` after
/// arming the yield signal; the scheduler delivers the real value on resume.
fn checkout_offload<T: Send + 'static>(
    op_name: &'static str,
    handle: String,
    op: impl FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
    decode: impl Fn(T) -> Value + 'static,
) -> Result<Value, SemaError> {
    use std::rc::Rc;
    use tokio::sync::oneshot::error::TryRecvError;

    // Vestigial under CALL_NATIVE (the scheduler delivers the resume value via
    // `replace_stack_top`, not by re-invoking this native), but kept for
    // symmetry with the shipped `async/await` yield pattern.
    if let Some(v) = sema_core::take_resume_value() {
        return Ok(v);
    }

    let phase = Rc::new(RefCell::new(ConnPhase::<T>::Acquire));
    let phase_for_poll = phase.clone();
    let mut op_holder = Some(op);
    let handle_for_poll = handle.clone();

    let poll = move || -> IoPoll {
        loop {
            let is_acquire = matches!(&*phase_for_poll.borrow(), ConnPhase::Acquire);
            if is_acquire {
                // Owned Result so the DB_CONNECTIONS borrow doesn't outlive
                // the match — the `Running` transition below needs its own
                // (non-overlapping) borrow of the same thread-local.
                let mut taken: Option<Result<Connection, String>> = None;
                DB_CONNECTIONS.with(|c| {
                    let mut conns = c.borrow_mut();
                    match conns.get_mut(&handle_for_poll) {
                        Some(slot @ DbSlot::Available(_)) => {
                            let DbSlot::Available(conn) =
                                std::mem::replace(slot, DbSlot::CheckedOut)
                            else {
                                unreachable!("just matched Available")
                            };
                            taken = Some(Ok(conn));
                        }
                        Some(DbSlot::CheckedOut) => {}
                        Some(DbSlot::Tombstone(msg)) => {
                            taken = Some(Err(
                                tombstone_err(op_name, &handle_for_poll, msg).to_string()
                            ));
                        }
                        None => {
                            taken = Some(Err(missing_err(op_name, &handle_for_poll).to_string()));
                        }
                    }
                });
                match taken {
                    None => return IoPoll::Pending,
                    Some(Err(msg)) => return IoPoll::Ready(Err(msg)),
                    Some(Ok(conn)) => {
                        let op = op_holder
                            .take()
                            .expect("checkout_offload's op is consumed exactly once");
                        *phase_for_poll.borrow_mut() = ConnPhase::Running(spawn_conn_op(conn, op));
                        // Fall through: poll the freshly spawned receiver
                        // immediately instead of wasting a scheduler tick.
                    }
                }
            } else {
                let mut phase_ref = phase_for_poll.borrow_mut();
                let ConnPhase::Running(rx) = &mut *phase_ref else {
                    unreachable!("Acquire handled above")
                };
                return match rx.try_recv() {
                    Err(TryRecvError::Empty) => IoPoll::Pending,
                    Ok(outcome) => {
                        drop(phase_ref);
                        DB_CONNECTIONS.with(|c| {
                            c.borrow_mut()
                                .insert(handle_for_poll.clone(), DbSlot::Available(outcome.conn))
                        });
                        // MANDATORY lost-wakeup guard: a sibling queued on this
                        // same handle (still in `Acquire`) may have polled
                        // Pending earlier in this scheduler sweep — without
                        // this it would park until an unrelated wakeup.
                        sema_core::notify_io_complete();
                        match outcome.result {
                            Ok(t) => IoPoll::Ready(Ok(decode(t))),
                            Err(msg) => IoPoll::Ready(Err(msg)),
                        }
                    }
                    Err(TryRecvError::Closed) => {
                        drop(phase_ref);
                        DB_CONNECTIONS.with(|c| {
                            c.borrow_mut().insert(
                                handle_for_poll.clone(),
                                DbSlot::Tombstone(
                                    "the query worker terminated unexpectedly".to_string(),
                                ),
                            )
                        });
                        IoPoll::Ready(Err(format!("{op_name}: query worker dropped")))
                    }
                };
            }
        }
    };

    let phase_for_abort = phase.clone();
    let handle_for_abort = handle;
    let io_handle = Rc::new(IoHandle::with_abort(poll, move || {
        // Acquire-phase abort: no-op — nothing was ever checked out, the
        // registry slot is exactly as another caller left it. Running-phase
        // abort: best-effort — see `spawn_conn_op`'s doc comment.
        if matches!(*phase_for_abort.borrow(), ConnPhase::Running(_)) {
            DB_CONNECTIONS.with(|c| {
                c.borrow_mut().insert(
                    handle_for_abort.clone(),
                    DbSlot::Tombstone(format!(
                        "{op_name} was cancelled while in flight; the connection cannot be \
                         reclaimed — db/close frees the handle"
                    )),
                );
            });
        }
    }));
    sema_core::set_yield_signal(sema_core::YieldReason::AwaitIo(io_handle));
    Ok(Value::nil())
}

/// Run `sql` against `conn` with `params`, collecting every row into owned
/// `(column, value)` pairs — the `Send` intermediate that crosses the offload
/// boundary. `Value`/map construction happens in `rows_to_value`/
/// `row_to_value`, on the VM thread, never inside the offload.
fn collect_query_rows(
    conn: &Connection,
    sql: &str,
    params: &[SqlValue],
) -> rusqlite::Result<Vec<Vec<(String, SqlValue)>>> {
    let mut stmt = conn.prepare(sql)?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap().to_string())
        .collect();
    let rows = stmt.query_map(params_from_iter(params.iter()), |row| {
        let mut r = Vec::with_capacity(col_count);
        for (i, name) in col_names.iter().enumerate() {
            let val: SqlValue = row.get(i)?;
            r.push((name.clone(), val));
        }
        Ok(r)
    })?;
    rows.collect()
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
) -> rusqlite::Result<Option<Vec<(String, SqlValue)>>> {
    let mut stmt = conn.prepare(sql)?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap().to_string())
        .collect();
    let mut rows = stmt.query_map(params_from_iter(params.iter()), |row| {
        let mut r = Vec::with_capacity(col_count);
        for (i, name) in col_names.iter().enumerate() {
            let val: SqlValue = row.get(i)?;
            r.push((name.clone(), val));
        }
        Ok(r)
    })?;
    rows.next().transpose()
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

fn collect_tables(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(names)
}

fn tables_to_value(names: Vec<String>) -> Value {
    Value::list(names.iter().map(|s| Value::string(s)).collect())
}

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (db/open path) or (db/open name path)
    crate::register_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/open",
        &[0],
        |args| {
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

            if in_async_context() {
                let key_for_decode = key.clone();
                return crate::io::fs_offload(
                    move || {
                        let conn = Connection::open(&path).map_err(|e| eval_msg("db/open", e))?;
                        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                            .map_err(|e| eval_msg("db/open", e))?;
                        Ok(conn)
                    },
                    move |conn| {
                        DB_CONNECTIONS.with(|c| {
                            c.borrow_mut()
                                .insert(key_for_decode.clone(), DbSlot::Available(conn))
                        });
                        Value::string(&key_for_decode)
                    },
                );
            }

            let conn =
                Connection::open(&path).map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                .map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
            DB_CONNECTIONS.with(|c| c.borrow_mut().insert(key.clone(), DbSlot::Available(conn)));
            Ok(Value::string(&key))
        },
    );

    // (db/open-memory) or (db/open-memory name)
    crate::register_fn_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/open-memory",
        |args| {
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

            if in_async_context() {
                let name_for_decode = name.clone();
                return crate::io::fs_offload(
                    move || {
                        let conn = Connection::open_in_memory()
                            .map_err(|e| eval_msg("db/open-memory", e))?;
                        conn.execute_batch("PRAGMA foreign_keys=ON;")
                            .map_err(|e| eval_msg("db/open-memory", e))?;
                        Ok(conn)
                    },
                    move |conn| {
                        DB_CONNECTIONS.with(|c| {
                            c.borrow_mut()
                                .insert(name_for_decode.clone(), DbSlot::Available(conn))
                        });
                        Value::string(&name_for_decode)
                    },
                );
            }

            let conn = Connection::open_in_memory()
                .map_err(|e| SemaError::eval(format!("db/open-memory: {e}")))?;
            conn.execute_batch("PRAGMA foreign_keys=ON;")
                .map_err(|e| SemaError::eval(format!("db/open-memory: {e}")))?;
            DB_CONNECTIONS.with(|c| c.borrow_mut().insert(name.clone(), DbSlot::Available(conn)));
            Ok(Value::string(&name))
        },
    );

    // (db/exec handle sql ...params) -> int (affected rows)
    crate::register_fn_gated(env, sandbox, sema_core::Caps::FS_WRITE, "db/exec", |args| {
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

        if in_async_context() {
            return checkout_offload(
                "db/exec",
                handle,
                move |conn| {
                    conn.execute(&sql, params_from_iter(params.iter()))
                        .map(|n| n as i64)
                        .map_err(|e| eval_msg("db/exec", e))
                },
                Value::int,
            );
        }

        with_conn("db/exec", &handle, |conn| {
            conn.execute(&sql, params_from_iter(params.iter()))
                .map(|n| Value::int(n as i64))
                .map_err(|e| SemaError::eval(format!("db/exec: {e}")))
        })
    });

    // (db/exec-batch handle sql) -> nil (execute multiple statements)
    // STATIC SQL ONLY: no parameter binding — the string is run verbatim.
    // Never interpolate user-controlled input; use parameterized db/exec for that.
    crate::register_fn_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/exec-batch",
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

            if in_async_context() {
                return checkout_offload(
                    "db/exec-batch",
                    handle,
                    move |conn| {
                        conn.execute_batch(&sql)
                            .map_err(|e| eval_msg("db/exec-batch", e))
                    },
                    |()| Value::nil(),
                );
            }

            with_conn("db/exec-batch", &handle, |conn| {
                conn.execute_batch(&sql)
                    .map(|()| Value::nil())
                    .map_err(|e| SemaError::eval(format!("db/exec-batch: {e}")))
            })
        },
    );

    // (db/query handle sql ...params) -> list of maps
    crate::register_fn_gated(env, sandbox, sema_core::Caps::FS_READ, "db/query", |args| {
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

        if in_async_context() {
            return checkout_offload(
                "db/query",
                handle,
                move |conn| {
                    collect_query_rows(conn, &sql, &params).map_err(|e| eval_msg("db/query", e))
                },
                rows_to_value,
            );
        }

        with_conn("db/query", &handle, |conn| {
            collect_query_rows(conn, &sql, &params)
                .map(rows_to_value)
                .map_err(|e| SemaError::eval(format!("db/query: {e}")))
        })
    });

    // (db/query-one handle sql ...params) -> map or nil
    crate::register_fn_gated(
        env,
        sandbox,
        sema_core::Caps::FS_READ,
        "db/query-one",
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

            if in_async_context() {
                return checkout_offload(
                    "db/query-one",
                    handle,
                    move |conn| {
                        collect_first_query_row(conn, &sql, &params)
                            .map_err(|e| eval_msg("db/query-one", e))
                    },
                    row_to_value,
                );
            }

            with_conn("db/query-one", &handle, |conn| {
                collect_first_query_row(conn, &sql, &params)
                    .map(row_to_value)
                    .map_err(|e| SemaError::eval(format!("db/query-one: {e}")))
            })
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
    crate::register_fn_gated(
        env,
        sandbox,
        sema_core::Caps::FS_READ,
        "db/tables",
        |args| {
            check_arity!(args, "db/tables", 1);
            let handle = args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string();

            if in_async_context() {
                return checkout_offload(
                    "db/tables",
                    handle,
                    move |conn| collect_tables(conn).map_err(|e| eval_msg("db/tables", e)),
                    tables_to_value,
                );
            }

            with_conn("db/tables", &handle, |conn| {
                collect_tables(conn)
                    .map(tables_to_value)
                    .map_err(|e| SemaError::eval(format!("db/tables: {e}")))
            })
        },
    );

    // (db/close handle) -> nil
    //
    // A handle checked out by an in-flight offload errors instead of racing
    // the background op for the same `Connection` (matches `proc/close`); a
    // missing or already-tombstoned handle is a silent no-op — `db/close`
    // remains the documented way to free either.
    crate::register_fn(env, "db/close", |args| {
        check_arity!(args, "db/close", 1);
        let handle = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        DB_CONNECTIONS.with(|c| {
            let mut conns = c.borrow_mut();
            if matches!(conns.get(handle), Some(DbSlot::CheckedOut)) {
                return Err(busy_err("db/close", handle));
            }
            conns.remove(handle);
            Ok(Value::nil())
        })
    });
}
