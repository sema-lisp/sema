//! Async-offload coverage for `db/*` (WP-DB).
//!
//! `crates/sema-stdlib/src/sqlite.rs` now branches on `in_async_context()`:
//! `db/open`/`db/open-memory` offload the connection open via `fs_offload`
//! (io.rs); `db/exec`/`db/exec-batch`/`db/query`/`db/query-one`/`db/tables`
//! offload the statement through a CHECKOUT registry slot
//! (`Available`/`CheckedOut`/`Tombstone`, see the module doc comment in
//! `sqlite.rs`) instead of blocking the VM thread (and every sibling task) on
//! `rusqlite::Connection::execute`/`prepare`/`query_map` for the call's whole
//! duration. At top level (no scheduler) every builtin keeps the original
//! synchronous shape.
//!
//! Every connection here is `:memory:` or a fresh temp file — no real disk
//! latency needed for these tests to be meaningful: the offload yields
//! `AwaitIo` the instant it's called (before the checkout's `spawn_blocking`
//! closure has any chance to run), so a zero-delay sibling task reliably
//! completes first — the same mechanism `proc_pty_async_test.rs` relies on
//! for `proc/wait`/`pty/wait`. Ordering is asserted via channel receive
//! order — never a wall-clock duration assert.

#![cfg(not(target_arch = "wasm32"))]

use sema_core::Value;
use sema_eval::Interpreter;

/// A unique temp DB file path for one test, removed (plus WAL/SHM sidecars)
/// on drop — also on panic.
struct TempDb(std::path::PathBuf);

impl TempDb {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("sema-db-async-{tag}-{nanos}.sqlite3"));
        TempDb(path)
    }
    fn path(&self) -> String {
        self.0.to_string_lossy().to_string()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(self.0.with_extension("sqlite3-shm"));
    }
}

// === Scheduler-not-stalled: a sibling task completes while a db/* op is in flight ===
//
// Pre-conversion, `db/exec`/`db/query` never yield, so the entire async task
// (open + create + insert + query + close) runs inside one uninterruptible
// scheduler step — "db" always wins the channel race. Post-conversion each
// checkout offload parks on `AwaitIo` the instant it's called, giving the
// zero-delay sibling a chance to run (and finish) first.
#[test]
fn db_async_lets_sibling_run_first() {
    let interp = Interpreter::new();
    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (db/open-memory "sib-order")
                (db/exec "sib-order" "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
                (db/exec "sib-order" "INSERT INTO t (v) VALUES (?)" "hello")
                (db/query "sib-order" "SELECT v FROM t")
                (db/close "sib-order")
                (channel/send out "db")))
              (async/spawn (fn () (channel/send out "sibling")))))
          (list (channel/recv out) (channel/recv out)))
    "#;
    let result = interp
        .eval_str_compiled(program)
        .expect("sibling-ordering program evaluated");
    let received: Vec<String> = result
        .as_list()
        .expect("channel receives list")
        .iter()
        .map(|v| v.as_str().expect("string value").to_string())
        .collect();
    assert_eq!(
        received,
        vec!["sibling".to_string(), "db".to_string()],
        "sibling task must complete while the offloaded db/* chain is in flight \
         (pre-conversion db/* always wins), got {received:?}"
    );
}

/// The full open/create/insert/query result inside `async/spawn` matches the
/// synchronous path exactly (same handle name reused sequentially, never
/// concurrently, so there is no checkout contention to reason about here).
#[test]
fn db_async_query_matches_sync() {
    let interp = Interpreter::new();
    let sync_v = interp
        .eval_str_compiled(
            r#"
            (db/open-memory "match-sync")
            (db/exec "match-sync" "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
            (db/exec "match-sync" "INSERT INTO t (v) VALUES (?)" "alpha")
            (db/exec "match-sync" "INSERT INTO t (v) VALUES (?)" "beta")
            (let ((rows (db/query "match-sync" "SELECT v FROM t ORDER BY v"))
                  (one (db/query-one "match-sync" "SELECT v FROM t WHERE v = ?" "alpha"))
                  (n (db/last-insert-id "match-sync"))
                  (tables (db/tables "match-sync")))
              (db/close "match-sync")
              (list rows one n tables))
            "#,
        )
        .expect("sync db chain");
    let async_v = interp
        .eval_str_compiled(
            r#"
            (await (async/spawn (fn ()
              (db/open-memory "match-async")
              (db/exec "match-async" "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
              (db/exec "match-async" "INSERT INTO t (v) VALUES (?)" "alpha")
              (db/exec "match-async" "INSERT INTO t (v) VALUES (?)" "beta")
              (let ((rows (db/query "match-async" "SELECT v FROM t ORDER BY v"))
                    (one (db/query-one "match-async" "SELECT v FROM t WHERE v = ?" "alpha"))
                    (n (db/last-insert-id "match-async"))
                    (tables (db/tables "match-async")))
                (db/close "match-async")
                (list rows one n tables)))))
            "#,
        )
        .expect("async db chain");
    assert_eq!(sync_v, async_v);
}

/// `db/open` (real file, not `:memory:`) offloads through `fs_offload` — a
/// distinct code path from the checkout-based statement ops. Proves the file
/// variant round-trips correctly inside async context.
#[test]
fn db_open_file_async_roundtrip() {
    let db = TempDb::new("open-file");
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (await (async/spawn (fn ()
          (db/open "{path}")
          (db/exec "{path}" "CREATE TABLE t (v TEXT)")
          (db/exec "{path}" "INSERT INTO t VALUES (?)" "hello")
          (let ((v (db/query-one "{path}" "SELECT v FROM t")))
            (db/close "{path}")
            v))))
        "#,
        path = db.path()
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("async db/open (file) chain");
    assert_eq!(
        result
            .as_map_ref()
            .and_then(|m| m.get(&Value::keyword("v")).cloned()),
        Some(Value::string("hello"))
    );
}

/// Regression: the async `db/query-one` offload must stop at the first row,
/// exactly like the sync path (`collect_first_query_row`, not
/// `collect_query_rows` + `remove(0)`). A later row that would raise a SQLite
/// runtime error (`abs(i64::MIN)` overflows) must never be evaluated.
#[test]
fn db_async_query_one_stops_at_first_row() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (await (async/spawn (fn ()
              (db/open-memory "qo-lazy-async")
              (db/exec "qo-lazy-async" "CREATE TABLE t (x INTEGER)")
              (db/exec "qo-lazy-async" "INSERT INTO t VALUES (1)")
              (db/exec "qo-lazy-async" "INSERT INTO t VALUES (-9223372036854775808)")
              (let ((row (db/query-one "qo-lazy-async" "SELECT abs(x) AS ax FROM t")))
                (db/close "qo-lazy-async")
                row))))
            "#,
        )
        .expect("async db/query-one must not evaluate the overflowing second row");
    assert_eq!(
        result
            .as_map_ref()
            .and_then(|m| m.get(&Value::keyword("ax")).and_then(|v| v.as_int())),
        Some(1)
    );
}

/// Three sibling tasks all call `db/exec` on the SAME handle concurrently.
/// Only one can hold the checkout at a time; the others must queue (the
/// `Acquire` phase re-attempting checkout each poll) rather than deadlock,
/// panic, or lose a write — proving the queued-caller path documented in
/// `sqlite.rs`.
#[test]
fn db_async_concurrent_writers_on_one_handle_all_succeed() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(
            r#"
            (let ((out (channel/new 8)))
              (db/open-memory "shared")
              (db/exec "shared" "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER)")
              (async/all
                (list
                  (async/spawn (fn ()
                    (db/exec "shared" "INSERT INTO t (v) VALUES (1)")
                    (channel/send out "a")))
                  (async/spawn (fn ()
                    (db/exec "shared" "INSERT INTO t (v) VALUES (2)")
                    (channel/send out "b")))
                  (async/spawn (fn ()
                    (db/exec "shared" "INSERT INTO t (v) VALUES (3)")
                    (channel/send out "c")))))
              (let ((r1 (channel/recv out))
                    (r2 (channel/recv out))
                    (r3 (channel/recv out))
                    (count (db/query-one "shared" "SELECT count(*) as n FROM t")))
                (db/close "shared")
                (list (sort (list r1 r2 r3)) count)))
            "#,
        )
        .expect("concurrent db/exec writers on one handle");
    let parts: Vec<Value> = result.as_list().expect("list").to_vec();
    assert_eq!(
        parts[0],
        Value::list(vec![
            Value::string("a"),
            Value::string("b"),
            Value::string("c")
        ]),
        "all three queued writers must complete, got {:?}",
        parts[0]
    );
    let count = parts[1]
        .as_map_ref()
        .and_then(|m| m.get(&Value::keyword("n")).and_then(|v| v.as_int()));
    assert_eq!(
        count,
        Some(3),
        "all three inserts must land — the checkout must serialize, not drop, queued writers"
    );
}
