use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use sema_core::{check_arity, SemaError, Value};

thread_local! {
    static DB_CONNECTIONS: RefCell<HashMap<String, Connection>> = RefCell::new(HashMap::new());
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

pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // (db/open path) or (db/open name path)
    crate::register_fn_path_gated(
        env,
        sandbox,
        sema_core::Caps::FS_WRITE,
        "db/open",
        &[0],
        |args| {
            if args.len() == 1 {
                let path = args[0]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
                let conn =
                    Connection::open(path).map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
                conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                    .map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
                DB_CONNECTIONS.with(|c| {
                    c.borrow_mut().insert(path.to_string(), conn);
                });
                Ok(Value::string(path))
            } else if args.len() == 2 {
                let name = args[0]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
                let path = args[1]
                    .as_str()
                    .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
                let conn =
                    Connection::open(path).map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
                conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
                    .map_err(|e| SemaError::eval(format!("db/open: {e}")))?;
                DB_CONNECTIONS.with(|c| {
                    c.borrow_mut().insert(name.to_string(), conn);
                });
                Ok(Value::string(name))
            } else {
                Err(SemaError::arity("db/open", "1 or 2", args.len()))
            }
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
            let conn = Connection::open_in_memory()
                .map_err(|e| SemaError::eval(format!("db/open-memory: {e}")))?;
            conn.execute_batch("PRAGMA foreign_keys=ON;")
                .map_err(|e| SemaError::eval(format!("db/open-memory: {e}")))?;
            DB_CONNECTIONS.with(|c| {
                c.borrow_mut().insert(name.clone(), conn);
            });
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
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sql = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let params: Vec<SqlValue> = args[2..].iter().map(sema_to_sql).collect();

        DB_CONNECTIONS.with(|c| {
            let c = c.borrow();
            let conn = c
                .get(handle)
                .ok_or_else(|| SemaError::eval(format!("db/exec: no open database '{handle}'")))?;
            let affected = conn
                .execute(sql, params_from_iter(params.iter()))
                .map_err(|e| SemaError::eval(format!("db/exec: {e}")))?;
            Ok(Value::int(affected as i64))
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
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let sql = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;

            DB_CONNECTIONS.with(|c| {
                let c = c.borrow();
                let conn = c.get(handle).ok_or_else(|| {
                    SemaError::eval(format!("db/exec-batch: no open database '{handle}'"))
                })?;
                conn.execute_batch(sql)
                    .map_err(|e| SemaError::eval(format!("db/exec-batch: {e}")))?;
                Ok(Value::nil())
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
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        let sql = args[1]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
        let params: Vec<SqlValue> = args[2..].iter().map(sema_to_sql).collect();

        DB_CONNECTIONS.with(|c| {
            let c = c.borrow();
            let conn = c
                .get(handle)
                .ok_or_else(|| SemaError::eval(format!("db/query: no open database '{handle}'")))?;
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| SemaError::eval(format!("db/query: {e}")))?;
            let col_count = stmt.column_count();
            let col_names: Vec<String> = (0..col_count)
                .map(|i| stmt.column_name(i).unwrap().to_string())
                .collect();

            let rows = stmt
                .query_map(params_from_iter(params.iter()), |row| {
                    let mut map = BTreeMap::new();
                    for (i, name) in col_names.iter().enumerate() {
                        let val: SqlValue = row.get(i)?;
                        map.insert(Value::keyword(name), sql_to_sema(&val));
                    }
                    Ok(Value::map(map))
                })
                .map_err(|e| SemaError::eval(format!("db/query: {e}")))?;

            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| SemaError::eval(format!("db/query: {e}")))?);
            }
            Ok(Value::list(result))
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
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
            let sql = args[1]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[1].type_name()))?;
            let params: Vec<SqlValue> = args[2..].iter().map(sema_to_sql).collect();

            DB_CONNECTIONS.with(|c| {
                let c = c.borrow();
                let conn = c.get(handle).ok_or_else(|| {
                    SemaError::eval(format!("db/query-one: no open database '{handle}'"))
                })?;
                let mut stmt = conn
                    .prepare(sql)
                    .map_err(|e| SemaError::eval(format!("db/query-one: {e}")))?;
                let col_count = stmt.column_count();
                let col_names: Vec<String> = (0..col_count)
                    .map(|i| stmt.column_name(i).unwrap().to_string())
                    .collect();

                let mut rows = stmt
                    .query_map(params_from_iter(params.iter()), |row| {
                        let mut map = BTreeMap::new();
                        for (i, name) in col_names.iter().enumerate() {
                            let val: SqlValue = row.get(i)?;
                            map.insert(Value::keyword(name), sql_to_sema(&val));
                        }
                        Ok(Value::map(map))
                    })
                    .map_err(|e| SemaError::eval(format!("db/query-one: {e}")))?;

                match rows.next() {
                    Some(row) => row.map_err(|e| SemaError::eval(format!("db/query-one: {e}"))),
                    None => Ok(Value::nil()),
                }
            })
        },
    );

    // (db/last-insert-id handle) -> int
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

            DB_CONNECTIONS.with(|c| {
                let c = c.borrow();
                let conn = c.get(handle).ok_or_else(|| {
                    SemaError::eval(format!("db/last-insert-id: no open database '{handle}'"))
                })?;
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
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;

            DB_CONNECTIONS.with(|c| {
            let c = c.borrow();
            let conn = c.get(handle).ok_or_else(|| {
                SemaError::eval(format!("db/tables: no open database '{handle}'"))
            })?;
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .map_err(|e| SemaError::eval(format!("db/tables: {e}")))?;
            let names: Vec<Value> = stmt
                .query_map([], |row| {
                    let name: String = row.get(0)?;
                    Ok(Value::string(&name))
                })
                .map_err(|e| SemaError::eval(format!("db/tables: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            Ok(Value::list(names))
        })
        },
    );

    // (db/close handle) -> nil
    crate::register_fn(env, "db/close", |args| {
        check_arity!(args, "db/close", 1);
        let handle = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        DB_CONNECTIONS.with(|c| {
            c.borrow_mut().remove(handle);
        });
        Ok(Value::nil())
    });
}
