//! Async-offload coverage for `kv/*` (WP-KV).
//!
//! `crates/sema-stdlib/src/kv.rs` now branches on `in_async_context()`:
//! `kv/open` offloads its initial read+parse through `fs_offload` (io.rs),
//! mirroring `db/open`; `kv/set`/`kv/delete` mutate the store's in-memory
//! `data` map on the VM thread (so the write is observable to a later
//! `kv/get` immediately), then offload the write-through flush (JSON encode +
//! `std::fs::write` of the WHOLE store) through a CHECKOUT registry slot
//! (`Available`/`CheckedOut`/`Tombstone`, see the module doc comment in
//! `kv.rs`) instead of blocking the VM thread — and every sibling task — for
//! the store's whole size on every single mutation. The call does not
//! resolve until the flush completes: durability (write-through, no lost
//! writes on crash) is preserved, only the WAIT moves off the VM thread. At
//! top level (no scheduler) every builtin keeps the original synchronous
//! shape.
//!
//! Every store here is a fresh temp file — no real disk latency needed for
//! these tests to be meaningful: the offload yields `AwaitIo` the instant
//! it's called (before the checkout's `spawn_blocking` closure has any
//! chance to run), so a zero-delay sibling task reliably completes first —
//! the same mechanism `db_async_test.rs`/`proc_pty_async_test.rs` rely on.
//! Ordering is asserted via channel receive order — never a wall-clock
//! duration assert.

#![cfg(not(target_arch = "wasm32"))]

use sema_core::Value;
use sema_eval::Interpreter;

/// A unique temp KV-store JSON path for one test, removed on drop — also on
/// panic.
struct TempKv(std::path::PathBuf);

impl TempKv {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("sema-kv-async-{tag}-{nanos}.json"));
        TempKv(path)
    }
    fn path(&self) -> String {
        self.0.to_string_lossy().to_string()
    }
}

impl Drop for TempKv {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// === Scheduler-not-stalled: a sibling task completes while a kv/set flush is
// === in flight, on a store big enough that the flush is a real whole-file
// === rewrite ===
//
// Pre-conversion, `kv/set`'s flush never yields, so the entire async task
// (set + channel/send) runs inside one uninterruptible scheduler step — "kv"
// always wins the channel race. Post-conversion the flush offload parks on
// `AwaitIo` the instant it's called, giving the zero-delay sibling a chance
// to run (and finish) first. The store is seeded with 500 keys (sync, before
// the race) so the flush inside the race is a genuine whole-store rewrite,
// not a near-empty write.
#[test]
fn kv_async_lets_sibling_run_first() {
    let interp = Interpreter::new();
    let kv = TempKv::new("sib-order");
    let path = kv.path();

    let mut seed_prog = format!(r#"(kv/open "sib-order" "{path}")"#);
    for i in 0..500 {
        seed_prog.push_str(&format!(r#"(kv/set "sib-order" "k{i}" {i})"#));
    }
    interp
        .eval_str_compiled(&seed_prog)
        .expect("seed store with 500 keys (sync)");

    let program = r#"
        (let ((out (channel/new 8)))
          (async/all
            (list
              (async/spawn (fn ()
                (kv/set "sib-order" "new-key" "new-value")
                (channel/send out "kv")))
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
        vec!["sibling".to_string(), "kv".to_string()],
        "sibling task must complete while the offloaded kv/set flush is in flight \
         (pre-conversion kv/set always wins), got {received:?}"
    );

    // The flush isn't fire-and-forget: by the time the program above
    // returned, the write MUST already be durable on disk. Re-open the same
    // path under a fresh registry name (a stand-in for a process restart)
    // and confirm both the new key and every seeded key survived.
    let check = interp
        .eval_str_compiled(&format!(
            r#"
            (kv/open "sib-order-check" "{path}")
            (let ((v (kv/get "sib-order-check" "new-key"))
                  (k0 (kv/get "sib-order-check" "k0"))
                  (k499 (kv/get "sib-order-check" "k499")))
              (kv/close "sib-order-check")
              (list v k0 k499))
            "#
        ))
        .expect("verify the flush landed on disk");
    let parts = check.as_list().expect("list").to_vec();
    assert_eq!(parts[0], Value::string("new-value"));
    assert_eq!(parts[1], Value::int(0));
    assert_eq!(parts[2], Value::int(499));
}

/// The full open/set/get/delete/keys result inside `async/spawn` matches the
/// synchronous path exactly (distinct stores, so there is no checkout
/// contention to reason about here) — proves the offloaded flush doesn't
/// change any observable value, only where the wait happens.
#[test]
fn kv_async_open_set_get_delete_matches_sync() {
    let interp = Interpreter::new();

    let kv_sync = TempKv::new("match-sync");
    let path_sync = kv_sync.path();
    let sync_v = interp
        .eval_str_compiled(&format!(
            r#"
            (kv/open "match-sync" "{path_sync}")
            (kv/set "match-sync" "name" "Alice")
            (kv/set "match-sync" "count" 42)
            (let ((got (kv/get "match-sync" "name"))
                  (existed (kv/delete "match-sync" "count"))
                  (missing (kv/get "match-sync" "count"))
                  (ks (kv/keys "match-sync")))
              (kv/close "match-sync")
              (list got existed missing ks))
            "#
        ))
        .expect("sync kv chain");

    let kv_async = TempKv::new("match-async");
    let path_async = kv_async.path();
    let async_v = interp
        .eval_str_compiled(&format!(
            r#"
            (await (async/spawn (fn ()
              (kv/open "match-async" "{path_async}")
              (kv/set "match-async" "name" "Alice")
              (kv/set "match-async" "count" 42)
              (let ((got (kv/get "match-async" "name"))
                    (existed (kv/delete "match-async" "count"))
                    (missing (kv/get "match-async" "count"))
                    (ks (kv/keys "match-async")))
                (kv/close "match-async")
                (list got existed missing ks)))))
            "#
        ))
        .expect("async kv chain");

    assert_eq!(sync_v, async_v);
    assert_eq!(
        interp.runtime_resource_gate_count(),
        0,
        "kv/close must return the runtime's gate registry to baseline"
    );
}

#[test]
fn kv_gate_created_via_runtime_closes_through_compiled_entrypoint() {
    let kv = TempKv::new("mixed-entry-close");
    let interp = Interpreter::new();
    interp
        .eval_str_via_runtime(&format!(
            r#"(kv/open "mixed-entry-close" "{}")
                (kv/set "mixed-entry-close" "k" "v")"#,
            kv.path()
        ))
        .expect("runtime entry creates and mutates store");
    assert_eq!(interp.runtime_resource_gate_count(), 1);

    let result = interp
        .eval_str_compiled(r#"(kv/close "mixed-entry-close")"#)
        .expect("compiled entry closes the runtime-created gate");
    assert!(result.is_nil());
    assert_eq!(interp.runtime_resource_gate_count(), 0);
}

#[test]
fn kv_close_from_foreign_runtime_closes_the_owner_gate() {
    let kv = TempKv::new("foreign-runtime-close");
    let owner = Interpreter::new();
    let caller = Interpreter::new();
    owner
        .eval_str_via_runtime(&format!(
            r#"(kv/open "foreign-runtime-close" "{}")
                (kv/set "foreign-runtime-close" "k" "v")"#,
            kv.path()
        ))
        .expect("owner runtime creates and uses KV store");
    assert_eq!(owner.runtime_resource_gate_count(), 1);
    assert_eq!(caller.runtime_resource_gate_count(), 0);

    let result = caller
        .eval_str_via_runtime(r#"(kv/close "foreign-runtime-close")"#)
        .expect("foreign runtime routes close through the gate owner");
    assert!(result.is_nil());
    assert_eq!(owner.runtime_resource_gate_count(), 0);
    assert_eq!(caller.runtime_resource_gate_count(), 0);
    let error = owner
        .eval_str_via_runtime(r#"(kv/get "foreign-runtime-close" "k")"#)
        .expect_err("accepted foreign close removes the KV resource");
    assert!(error.to_string().contains("not open"), "{error}");
}

/// `kv/open` on a brand-new path offloads through `fs_offload` — a distinct
/// code path from the checkout-based `kv/set`/`kv/delete` flush. Proves the
/// "doesn't exist yet -> empty store" branch round-trips correctly inside
/// async context.
#[test]
fn kv_open_async_creates_new_store() {
    let interp = Interpreter::new();
    let kv = TempKv::new("open-new");
    let path = kv.path();
    assert!(!std::path::Path::new(&path).exists());

    let program = format!(
        r#"
        (await (async/spawn (fn ()
          (kv/open "open-new" "{path}")
          (kv/set "open-new" "k" "v")
          (let ((v (kv/get "open-new" "k")))
            (kv/close "open-new")
            v))))
        "#
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("async kv/open (new path) chain");
    assert_eq!(result, Value::string("v"));
    assert!(
        std::path::Path::new(&path).exists(),
        "kv/close's flush must have created the backing file"
    );
}

/// Three sibling tasks all call `kv/set` on the SAME store concurrently. Only
/// one can hold the checkout at a time; the others must queue (the `Acquire`
/// phase re-attempting checkout each poll) rather than deadlock, panic, or
/// silently lose a write — proving the queued-caller path documented in
/// `kv.rs`.
#[test]
fn kv_async_concurrent_writers_on_one_store_all_succeed() {
    let interp = Interpreter::new();
    let kv = TempKv::new("queued");
    let path = kv.path();
    let result = interp
        .eval_str_compiled(&format!(
            r#"
            (let ((out (channel/new 8)))
              (kv/open "shared" "{path}")
              (async/all
                (list
                  (async/spawn (fn () (kv/set "shared" "a" 1) (channel/send out "a")))
                  (async/spawn (fn () (kv/set "shared" "b" 2) (channel/send out "b")))
                  (async/spawn (fn () (kv/set "shared" "c" 3) (channel/send out "c")))))
              (let ((r1 (channel/recv out))
                    (r2 (channel/recv out))
                    (r3 (channel/recv out))
                    (ks (sort (kv/keys "shared"))))
                (kv/close "shared")
                (list (sort (list r1 r2 r3)) ks)))
            "#
        ))
        .expect("concurrent kv/set writers on one store");
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
    assert_eq!(
        parts[1],
        Value::list(vec![
            Value::string("a"),
            Value::string("b"),
            Value::string("c")
        ]),
        "all three keys must have landed — the checkout must serialize, not drop, queued writers, \
         got {:?}",
        parts[1]
    );

    // Re-open from disk to confirm the LAST flush to land actually persisted
    // all three keys (not just the in-memory copy the reader above saw).
    let on_disk = interp
        .eval_str_compiled(&format!(
            r#"
            (kv/open "queued-check" "{path}")
            (let ((ks (sort (kv/keys "queued-check"))))
              (kv/close "queued-check")
              ks)
            "#
        ))
        .expect("re-read store from disk");
    assert_eq!(
        on_disk,
        Value::list(vec![
            Value::string("a"),
            Value::string("b"),
            Value::string("c")
        ]),
        "the on-disk file must reflect every queued write, got {on_disk:?}"
    );
}

// === Cancellation through the ResourceGate + checkout_external path ===
//
// Cancelling a spawned kv chain must settle Cancelled (never hang or panic) and
// leave the registry usable: a fresh store opened afterwards works normally.
#[test]
fn kv_cancelled_chain_settles_and_registry_stays_usable() {
    let dir = std::env::temp_dir();
    let p1 = dir.join(format!("sema-kv-cancel-a-{}.json", std::process::id()));
    let p2 = dir.join(format!("sema-kv-cancel-b-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&p1);
    let _ = std::fs::remove_file(&p2);
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (let ((p (async/spawn (fn ()
                    (kv/open "cancelme" "{a}")
                    (kv/set "cancelme" "k" "v")))))
          (async/cancel p)
          (let ((caught (try (async/await p) (catch e :caught))))
            (kv/open "after" "{b}")
            (kv/set "after" "x" "ok")
            (let ((got (kv/get "after" "x")))
              (kv/close "after")
              (list caught got))))
        "#,
        a = p1.display(),
        b = p2.display(),
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("cancelled kv chain evaluates without wedging the runtime");
    let parts: Vec<Value> = result.as_list().expect("list").to_vec();
    assert_eq!(parts[0], Value::keyword("caught"));
    assert_eq!(parts[1], Value::string("ok"));
    assert_eq!(interp.runtime_resource_gate_count(), 0);
    let _ = std::fs::remove_file(&p1);
    let _ = std::fs::remove_file(&p2);
}

// A cancelled sibling (settled Cancelled pre-run) must not corrupt a shared
// store: the other two writers still acquire the gate FIFO and both writes land.
#[test]
fn kv_cancelled_sibling_does_not_corrupt_shared_store() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("sema-kv-contend-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let interp = Interpreter::new();
    let program = format!(
        r#"
        (kv/open "contend" "{p}")
        (let ((mid (async/spawn (fn () (kv/set "contend" "mid" 0)))))
          (async/cancel mid)
          (let ((pa (async/spawn (fn () (kv/set "contend" "a" 1))))
                (pc (async/spawn (fn () (kv/set "contend" "c" 3)))))
            (async/await pa)
            (async/await pc)
            (let ((keys (kv/keys "contend")))
              (kv/close "contend")
              (length keys))))
        "#,
        p = path.display(),
    );
    let result = interp
        .eval_str_compiled(&program)
        .expect("cancelled sibling evaluates without hanging or corrupting the store");
    assert_eq!(
        result.as_int(),
        Some(2),
        "both non-cancelled writers must land; the cancelled one must not"
    );
    assert_eq!(interp.runtime_resource_gate_count(), 0);
    let _ = std::fs::remove_file(&path);
}

// === Persistence bounds (B2) ===
//
// `kv/open` rejects an oversized backing file — pre-dispatch on the runtime
// path (metadata preflight, no allocation) and via the capped read on the sync
// path. Store bounds are lowered through the test-only override so the file need
// not actually reach the 64 MiB shipped ceiling.
#[test]
fn kv_open_rejects_oversized_store() {
    let interp = Interpreter::new();
    let kv = TempKv::new("oversized");
    let path = kv.path();
    std::fs::write(kv.path(), vec![b'x'; 4096]).expect("seed oversized backing file");
    sema_stdlib::set_kv_bounds_override(Some((1024, 1_000_000)));

    // Sync top-level open: rejected by the capped read.
    let sync_err = interp
        .eval_str_compiled(&format!(r#"(kv/open "oversized" "{path}")"#))
        .expect_err("oversized store must be rejected at kv/open");
    assert!(
        sync_err.to_string().contains("kv store limit"),
        "sync open error: {sync_err}"
    );

    // Runtime (async) open: rejected pre-dispatch by the metadata preflight.
    let async_msg = interp
        .eval_str_compiled(&format!(
            r#"(try (async/await (async/spawn (fn () (kv/open "oversized-async" "{path}"))))
                 (catch e (:message e)))"#
        ))
        .expect("runtime open try/catch resolves");
    assert!(
        async_msg
            .as_str()
            .is_some_and(|m| m.contains("kv store limit")),
        "runtime open error: {async_msg:?}"
    );

    sema_stdlib::set_kv_bounds_override(None);
}

// An over-cap `kv/set` (a value whose serialized form alone exceeds the
// whole-store byte cap) fails cleanly — the store keeps its earlier keys, stays
// usable, and nothing over-cap lands on disk.
#[test]
fn kv_set_over_cap_value_fails_with_store_intact() {
    let interp = Interpreter::new();
    let kv = TempKv::new("set-over-cap");
    let path = kv.path();
    sema_stdlib::set_kv_bounds_override(Some((64, 1_000_000)));

    let big = "x".repeat(256);
    let result = interp
        .eval_str_compiled(&format!(
            r#"
            (kv/open "cap" "{path}")
            (kv/set "cap" "ok" "hello")
            (let ((caught (try (kv/set "cap" "big" "{big}") (catch e (:message e))))
                  (kept (kv/get "cap" "ok"))
                  (missing (kv/get "cap" "big"))
                  (ks (kv/keys "cap")))
              (kv/close "cap")
              (list caught kept missing ks))
            "#
        ))
        .expect("over-cap set is a clean error, not a wedge");
    let parts: Vec<Value> = result.as_list().expect("list").to_vec();
    assert!(
        parts[0]
            .as_str()
            .is_some_and(|m| m.contains("kv store limit")),
        "over-cap set error message: {:?}",
        parts[0]
    );
    assert_eq!(
        parts[1],
        Value::string("hello"),
        "existing key must survive"
    );
    assert!(parts[2].is_nil(), "the over-cap key must not have landed");
    assert_eq!(
        parts[3],
        Value::list(vec![Value::string("ok")]),
        "keys must be unchanged by the rejected set"
    );

    // The on-disk store reflects only the in-cap write (nothing oversized was
    // ever flushed), read back under the full shipped bounds.
    sema_stdlib::set_kv_bounds_override(None);
    let on_disk = interp
        .eval_str_compiled(&format!(
            r#"(kv/open "cap-check" "{path}")
               (let ((ks (kv/keys "cap-check"))) (kv/close "cap-check") ks)"#
        ))
        .expect("re-read store from disk");
    assert_eq!(on_disk, Value::list(vec![Value::string("ok")]));
}
