//! CORE-2 cycle-collector stress suite (ADR #66, plan §6 M3 adversarial step:
//! `docs/plans/2026-07-02-core2-gc.md`). Every test here tries to make the
//! collector FREE LIVE DATA or crash the runtime: workloads build garbage
//! cycles next to live ones, then call the live closures across/after
//! collections and pin exact results. A wrong value, an unbound-variable
//! error, or a panic means a live cell/env was severed.
//!
//! Leak *rates* are leak_test.rs's business; here `:collected > 0` assertions
//! only prove garbage-cycle reclamation actually goes through the real VM
//! wiring (payload tracer, upvalue-cell trace/sever, env severing).
//!
//! Determinism: no timing, no network. The candidate registry is thread-local
//! and survives across tests on one harness thread, so tests that assert
//! exact `:collected` behavior first run a quiescing `(gc/collect)` and keep
//! closure churn far below the 1024-entry growth threshold (no implicit
//! `make_closure` collection can intervene). Tests that WANT implicit
//! collections churn far past the threshold instead.

use sema_core::Value;
use sema_eval::Interpreter;
use std::path::PathBuf;

fn eval_ok(input: &str) -> Value {
    Interpreter::new()
        .eval_str_compiled(input)
        .unwrap_or_else(|e| panic!("eval failed for `{input}`: {e}"))
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sema-gc-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_file(dir: &std::path::Path, name: &str, src: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, src).expect("write module file");
    p.to_string_lossy().to_string()
}

// ── gc/collect + gc/stats surface ─────────────────────────────────

#[test]
fn gc_builtins_report_sane_stats() {
    let v = eval_ok(
        "(begin
           (define s (gc/collect))
           (define t (gc/stats))
           (list (integer? (:candidates s)) (integer? (:traced s))
                 (integer? (:collected s)) (integer? (:pruned s))
                 (integer? (:registry-size t)) (>= (:registry-size t) 0)))",
    );
    assert_eq!(v, eval_ok("'(true true true true true true)"));
}

#[test]
fn gc_builtins_reject_arguments() {
    let v = eval_ok(
        "(list (try (gc/collect 1) (catch e :err))
               (try (gc/stats 1) (catch e :err)))",
    );
    assert_eq!(
        v,
        Value::list(vec![Value::keyword("err"), Value::keyword("err")])
    );
}

#[test]
fn quiescent_double_collect_frees_nothing() {
    // No closures are created between the two passes, so a second collection
    // must find zero garbage — collecting twice can never sever more.
    let v = eval_ok("(begin (gc/collect) (= 0 (:collected (gc/collect))))");
    assert_eq!(v, Value::bool(true));
}

// ── Mutual local recursion: garbage collected, live pair untouched ─

#[test]
fn garbage_mutual_pairs_collected_while_live_pair_still_callable() {
    // Two cells, neither a self-capture — the shape that defeats any
    // weak-self-edge scheme. Three garbage pairs are reclaimed; the live
    // pair (same shape, still referenced) must answer correctly afterwards.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (make-pair)
             (define (ev? n) (if (<= n 0) true (od? (- n 1))))
             (define (od? n) (if (<= n 0) false (ev? (- n 1))))
             ev?)
           (define live (make-pair))
           (make-pair) (make-pair) (make-pair)
           (define freed (:collected (gc/collect)))
           (list (live 6) (live 7) (> freed 0)))",
    );
    assert_eq!(
        v,
        Value::list(vec![
            Value::bool(true),
            Value::bool(false),
            Value::bool(true)
        ])
    );
}

#[test]
fn live_pair_survives_interleaved_collect_churn() {
    // (gc/collect) after every garbage pair: 25 passes over a registry that
    // always contains the live pair — none may sever its cells.
    let v = eval_ok(
        "(begin
           (define (make-pair)
             (define (ev? n) (if (<= n 0) true (od? (- n 1))))
             (define (od? n) (if (<= n 0) false (ev? (- n 1))))
             ev?)
           (define live (make-pair))
           (define (churn i)
             (if (<= i 0)
                 :done
                 (begin (make-pair) (gc/collect) (churn (- i 1)))))
           (churn 25)
           (list (live 8) (live 9)))",
    );
    assert_eq!(v, Value::list(vec![Value::bool(true), Value::bool(false)]));
}

#[test]
fn threshold_autocollect_under_churn_keeps_live_pair_and_bounds_registry() {
    // 5000 garbage pairs blow well past the 1024-entry growth threshold, so
    // the make_closure safe point collects repeatedly MID-VM (no explicit
    // gc). The live pair must survive every implicit pass, and the registry
    // must stay bounded (survivor-derived threshold, not linear growth).
    let v = eval_ok(
        "(begin
           (define (make-pair)
             (define (ev? n) (if (<= n 0) true (od? (- n 1))))
             (define (od? n) (if (<= n 0) false (ev? (- n 1))))
             ev?)
           (define live (make-pair))
           (define (churn i)
             (if (<= i 0)
                 :done
                 (begin (make-pair) (churn (- i 1)))))
           (churn 5000)
           (list (live 7) (live 8) (< (:registry-size (gc/stats)) 5000)))",
    );
    assert_eq!(
        v,
        Value::list(vec![
            Value::bool(false),
            Value::bool(true),
            Value::bool(true)
        ])
    );
}

// ── Upvalue cells: open and closed, set! across collections ───────

#[test]
fn open_upvalue_set_bang_across_midframe_collections() {
    // The captures are OPEN (the defining frame is still executing) at every
    // (gc/collect). An open cell must never be severed — the closures keep
    // reading/writing the live stack slot across passes.
    let v = eval_ok(
        "(begin
           (define (test)
             (define x 10)
             (define (get) x)
             (define (bump) (set! x (+ x 1)))
             (gc/collect)
             (bump)
             (gc/collect)
             (bump)
             (list (get) x))
           (test))",
    );
    assert_eq!(v, Value::list(vec![Value::int(12), Value::int(12)]));
}

#[test]
fn closed_upvalue_counter_state_survives_collect_hammer() {
    // After `counter` returns, the cell is CLOSED and shared by the returned
    // closure. Ten collections interleaved with calls: the cell's state must
    // persist (a severed cell would reset to nil and crash the +).
    let v = eval_ok(
        "(begin
           (define (counter)
             (define n 0)
             (define (inc!) (set! n (+ n 1)) n)
             inc!)
           (define c (counter))
           (define (hammer i)
             (if (<= i 0)
                 :done
                 (begin (gc/collect) (c) (hammer (- i 1)))))
           (hammer 10)
           (c))",
    );
    assert_eq!(v, Value::int(11));
}

#[test]
fn set_cell_cycle_garbage_collected_live_copy_callable() {
    // set!-through-cell in both directions: `grab`'s cycle (cell rebound to
    // the closure itself) becomes garbage and is reclaimed; separately, a
    // stored COPY of a closure stays callable after its own-name cell is
    // rebound to a non-function and a collection runs.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (churn)
             (define box nil)
             (define (grab) box)
             (set! box grab)
             nil)
           (churn)
           (define freed (:collected (gc/collect)))
           (define (mk)
             (define (f n) (list :orig n))
             (define g f)
             (set! f 42)
             g)
           (define kept (mk))
           (gc/collect)
           (list (> freed 0) (kept 5)))",
    );
    assert_eq!(
        v,
        Value::list(vec![
            Value::bool(true),
            Value::list(vec![Value::keyword("orig"), Value::int(5)])
        ])
    );
}

// ── Cycles through channels and promises ───────────────────────────

#[test]
fn closure_smuggled_through_captured_channel_garbage_collected() {
    // f captures ch AND sits in ch's buffer: the cycle passes through the
    // channel's severable buffer. Once the function returns nothing outside
    // the cycle references either — reclaimed.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (smuggle-garbage)
             (define ch (channel/new 4))
             (define (f n) (if (<= n 0) ch (f (- n 1))))
             (channel/send ch f)
             nil)
           (smuggle-garbage)
           (> (:collected (gc/collect)) 0))",
    );
    assert_eq!(v, Value::bool(true));
}

#[test]
fn closure_smuggled_through_captured_channel_live_after_collect() {
    // Same cycle shape, but the channel is still reachable (global binding):
    // the collection must keep the whole cycle. Draining the channel and
    // calling the smuggled closure must yield the captured channel back.
    let v = eval_ok(
        "(begin
           (define (smuggle)
             (define ch (channel/new 4))
             (define (f n) (if (<= n 0) ch (f (- n 1))))
             (channel/send ch f)
             ch)
           (define ch2 (smuggle))
           (gc/collect)
           (define g (channel/recv ch2))
           (channel? (g 3)))",
    );
    assert_eq!(v, Value::bool(true));
}

#[test]
fn promise_channel_closure_cycle_live_handle_works_after_collect() {
    // promise → Resolved(r) → cell(ch) → ch → buffered promise: a cycle
    // through TWO severable cells (promise state + channel buffer). The
    // returned closure is the only external handle; after a collection it
    // must still walk the whole cycle (call → channel → recv promise →
    // re-await → call again).
    let v = eval_ok(
        "(begin
           (define (mk)
             (define ch (channel/new 2))
             (define p (async/spawn (fn ()
               (define (r n) (if (<= n 0) ch (r (- n 1))))
               r)))
             (define res (await p))
             (channel/send ch p)
             res)
           (define kept (mk))
           (gc/collect)
           (define ch-back (kept 3))
           (define p-back (channel/recv ch-back))
           (define r-again (await p-back))
           (channel? (r-again 0)))",
    );
    assert_eq!(v, Value::bool(true));
}

// ── Async: collections inside tasks and mid-exchange ──────────────

#[test]
fn spawned_tasks_with_recursive_locals_collect_inside_and_after() {
    // Each task builds a recursive local closure (a fresh cycle) and runs a
    // full collection from INSIDE the task VM; parked/running task state must
    // pin everything live. A final collection after all awaits stays sound.
    let v = eval_ok(
        "(begin
           (define (task-body id)
             (define (r n) (if (<= n 0) id (r (- n 1))))
             (gc/collect)
             (r 10))
           (define ps (list (async/spawn (fn () (task-body 1)))
                            (async/spawn (fn () (task-body 2)))
                            (async/spawn (fn () (task-body 3)))))
           (define results (map (fn (p) (await p)) ps))
           (gc/collect)
           results)",
    );
    assert_eq!(
        v,
        Value::list(vec![Value::int(1), Value::int(2), Value::int(3)])
    );
}

#[test]
fn channel_ping_pong_with_collects_mid_exchange() {
    // Two tasks exchange values over channels; each round both sides spin up
    // a garbage recursive closure and run a collection while the OTHER task
    // is parked on a channel op — parked-task stacks and cells must stay
    // intact across every pass.
    let v = eval_ok(
        "(begin
           (define ping (channel/new 2))
           (define pong (channel/new 2))
           (define (spin-cycle k)
             (define (r n) (if (<= n 0) k (r (- n 1))))
             (r k))
           (define (relay-step n)
             (if (<= n 0)
                 :done
                 (let ((v (channel/recv ping)))
                   (spin-cycle 5)
                   (gc/collect)
                   (channel/send pong (+ v 1))
                   (relay-step (- n 1)))))
           (define (drive-step n acc)
             (if (<= n 0)
                 acc
                 (begin
                   (spin-cycle 7)
                   (channel/send ping acc)
                   (gc/collect)
                   (drive-step (- n 1) (channel/recv pong)))))
           (define p1 (async/spawn (fn () (relay-step 3))))
           (define p2 (async/spawn (fn () (drive-step 3 100))))
           (list (await p2) (await p1)))",
    );
    assert_eq!(
        v,
        Value::list(vec![Value::int(103), Value::keyword("done")])
    );
}

// ── Collections from inside foreign frames ─────────────────────────

#[test]
fn collect_inside_hof_callback_with_set_bang() {
    // The callback runs as a nested frame on the live VM (C1 path); the
    // enclosing frame's locals are captured through OPEN cells while
    // (gc/collect) runs per element. `set!` must keep flowing back.
    let v = eval_ok(
        "(begin
           (define (outer)
             (define acc 0)
             (define (bump! x) (set! acc (+ acc x)))
             (define result (map (fn (x) (begin (gc/collect) (bump! x) (* x 2)))
                                 (list 1 2 3 4)))
             (list result acc))
           (outer))",
    );
    assert_eq!(
        v,
        Value::list(vec![
            Value::list(vec![
                Value::int(2),
                Value::int(4),
                Value::int(6),
                Value::int(8)
            ]),
            Value::int(10)
        ])
    );
}

#[test]
fn collect_inside_catch_after_unwind() {
    // Unwinding through the frame leaves its upvalue cells live; a collection
    // in the handler must not disturb them.
    let v = eval_ok(
        "(begin
           (define (protected)
             (define x 1)
             (define (get) x)
             (try
               (begin (set! x 2) (throw :boom))
               (catch e (begin (gc/collect) (get)))))
           (protected))",
    );
    assert_eq!(v, Value::int(2));
}

#[test]
fn collect_inside_macro_expansion() {
    // Macro bodies run on a nested VM at compile time; a collection during
    // expansion must leave the expander and the lowering pipeline intact.
    let v = eval_ok(
        "(begin
           (defmacro gcm (x) (begin (gc/collect) x))
           (define (uses) (gcm 42))
           (uses))",
    );
    assert_eq!(v, Value::int(42));
}

#[test]
fn collect_inside_sort_comparator_and_eval() {
    // Two more foreign-frame entry points: the sort-by key callback and a
    // nested `(eval …)` (the __vm-eval delegate) both host a collection while
    // a live recursive closure sits in the globals.
    let v = eval_ok(
        "(begin
           (define (mk k)
             (define (r n) (if (<= n 0) k (r (- n 1))))
             r)
           (define live (mk 99))
           (define (key-of x) (begin (gc/collect) (- 0 x)))
           (define sorted (sort-by key-of (list 3 1 2)))
           (eval (quote (begin (gc/collect) 7)))
           (list sorted (live 5)))",
    );
    assert_eq!(
        v,
        Value::list(vec![
            Value::list(vec![Value::int(3), Value::int(2), Value::int(1)]),
            Value::int(99)
        ])
    );
}

#[test]
fn named_let_loop_with_collect_in_tail() {
    // Named let lowers to a self-referential local closure — the same cycle
    // shape as a recursive define, collected/kept through the same cells.
    let v = eval_ok(
        "(let loop ((n 5) (acc 0))
           (if (<= n 0) (begin (gc/collect) acc) (loop (- n 1) (+ acc n))))",
    );
    assert_eq!(v, Value::int(15));
}

// ── Cycles through thunks ───────────────────────────────────────────

#[test]
fn thunk_closure_cycle_garbage_collected_live_forced_kept() {
    // `t = (delay (grab))` and `grab` returns t: forcing memoizes
    // `t.forced = t`, a cycle through the thunk's severable cell plus the
    // closure's capture. The garbage instance is discovered via the closure
    // candidate (thunk constructors register nothing); the live instance must
    // keep forcing to itself after a collection.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (mk-garbage)
             (define t (delay (grab)))
             (define (grab) t)
             (force t)
             nil)
           (mk-garbage)
           (define freed (:collected (gc/collect)))
           (define (mk-live)
             (define t (delay (grab)))
             (define (grab) t)
             (force t))
           (define kept (mk-live))
           (gc/collect)
           (list (> freed 0) (= kept (force kept))))",
    );
    assert_eq!(v, Value::list(vec![Value::bool(true), Value::bool(true)]));
}

// ── Closures held in collections ───────────────────────────────────

#[test]
fn deep_closure_capture_chain_live_and_garbage() {
    // 1000 closures each capturing the previous one: tracing hops
    // NativeFn → payload → closure → cell → NativeFn per link, so the
    // worklist (not native recursion) must carry the depth. The live chain
    // returns its full length before and after a 500-link garbage chain
    // (hung off a recursive closure) is reclaimed.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (make-chain n)
             (if (<= n 0)
                 (fn () 0)
                 (let ((next (make-chain (- n 1))))
                   (fn () (+ 1 (next))))))
           (define chain (make-chain 1000))
           (gc/collect)
           (define total (chain))
           (define (mk-garbage-chain)
             (define c (make-chain 500))
             (define (r k) (if (<= k 0) c (r (- k 1))))
             nil)
           (mk-garbage-chain)
           (list total (> (:collected (gc/collect)) 500) (chain)))",
    );
    assert_eq!(
        v,
        Value::list(vec![Value::int(1000), Value::bool(true), Value::int(1000)])
    );
}

#[test]
fn closures_in_record_fields_survive_collect() {
    // Record fields are interior (immutable) trace segments; closures stored
    // there must be kept and callable after a collection.
    let v = eval_ok(
        "(begin
           (define-record-type point (make-point x y) point? (x point-x) (y point-y))
           (define (mk k)
             (define (r n) (if (<= n 0) k (r (- n 1))))
             r)
           (define p (make-point (mk 1) (mk 2)))
           (gc/collect)
           (list ((point-x p) 3) ((point-y p) 4)))",
    );
    assert_eq!(v, Value::list(vec![Value::int(1), Value::int(2)]));
}

#[test]
fn closures_in_vector_and_map_survive_collect_hammer() {
    // Live recursive closures reachable only through container elements
    // (vector / map values) while 30 collections run interleaved with fresh
    // garbage cycles.
    let v = eval_ok(
        "(begin
           (define (mk k)
             (define (r n) (if (<= n 0) k (r (- n 1))))
             r)
           (define held-vec [(mk 1) (mk 2) (mk 3)])
           (define held-map {:a (mk 10) :b (mk 20)})
           (define (hammer i)
             (if (<= i 0)
                 :done
                 (begin (mk i) (gc/collect) (hammer (- i 1)))))
           (hammer 30)
           (list ((first held-vec) 2) ((get held-map :b) 4)))",
    );
    assert_eq!(v, Value::list(vec![Value::int(1), Value::int(20)]));
}

// ── Modules ─────────────────────────────────────────────────────────

#[test]
fn module_import_collect_then_exported_fn_and_stateful_counter() {
    // The module runs (gc/collect) at its own top level (module env is the
    // executing VM's globals mid-load), the importer collects again, then the
    // exported fn must still reach its private helper via home globals and a
    // counter closure must keep its cell state across further collections.
    let dir = temp_dir("mod-import");
    let m = write_file(
        &dir,
        "mod.sema",
        "(define (private-helper x) (* x 10))\n\
         (define (make-counter)\n\
           (define n 0)\n\
           (define (inc!) (set! n (+ n 1)) n)\n\
           inc!)\n\
         (define (public-api x) (private-helper (+ x 1)))\n\
         (gc/collect)\n",
    );
    let v = eval_ok(&format!(
        r#"(begin
             (import "{m}" public-api make-counter)
             (gc/collect)
             (define c (make-counter))
             (c) (c)
             (gc/collect)
             (list (public-api 3) (c)))"#
    ));
    assert_eq!(v, Value::list(vec![Value::int(40), Value::int(3)]));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Deep structures (worklist tracing, no native-stack recursion) ──

#[test]
fn deep_nested_list_garbage_collected_without_crash_live_kept() {
    // A 3000-deep nested list hangs off a garbage recursive closure's cell:
    // MarkGray/Scan/CollectWhite must walk it with worklists (a per-level
    // native frame would overflow). The same structure held live must come
    // back intact — full depth verified by walking it.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (nest n acc) (if (<= n 0) acc (nest (- n 1) (list acc))))
           (define (depth v acc) (if (list? v) (depth (first v) (+ acc 1)) acc))
           (define (mk-garbage)
             (define deep (nest 3000 42))
             (define (r k) (if (<= k 0) deep (r (- k 1))))
             nil)
           (mk-garbage)
           (define freed (:collected (gc/collect)))
           (define (mk-live)
             (define deep (nest 3000 42))
             (define (r k) (if (<= k 0) deep (r (- k 1))))
             r)
           (define keeper (mk-live))
           (gc/collect)
           (list (> freed 3000) (depth (keeper 5) 0)))",
    );
    assert_eq!(v, Value::list(vec![Value::bool(true), Value::int(3000)]));
}

// ── Interpreter teardown collections vs values that escaped ────────

#[test]
fn kept_closure_survives_interpreter_drop_collection() {
    // Interpreter::drop releases its env ref and runs an UNPINNED collection.
    // `kept` is the only external ref into the dead interpreter's global env;
    // trial deletion must see that count and keep the env (and `helper`)
    // alive, or this call resolves nothing.
    let kept = {
        let interp = Interpreter::new();
        interp
            .eval_str_compiled(
                "(begin (define (helper x) (* x 2)) (define (f x) (helper (+ x 1))) f)",
            )
            .expect("define f")
    };
    let interp2 = Interpreter::new();
    let out = sema_eval::call_value(&interp2.ctx, &kept, &[Value::int(20)]).expect("call kept");
    assert_eq!(out, Value::int(42));
    interp2.eval_str_compiled("(gc/collect)").expect("collect");
    let out2 = sema_eval::call_value(&interp2.ctx, &kept, &[Value::int(0)]).expect("recall kept");
    assert_eq!(out2, Value::int(2));
}

#[test]
fn kept_module_closure_survives_interpreter_drop() {
    // Same shape through a MODULE env: after the importing interpreter (and
    // its module cache) dies, the exported closure's home globals is the only
    // path keeping the module env — and the private helper — alive.
    let dir = temp_dir("mod-drop");
    let m = write_file(
        &dir,
        "lib.sema",
        "(define (private-helper x) (* x 10))\n(define (public-api x) (private-helper (+ x 1)))",
    );
    let kept = {
        let interp = Interpreter::new();
        interp
            .eval_str_compiled(&format!(r#"(begin (import "{m}" public-api) public-api)"#))
            .expect("import module")
    };
    let interp2 = Interpreter::new();
    let out = sema_eval::call_value(&interp2.ctx, &kept, &[Value::int(3)]).expect("call module fn");
    assert_eq!(out, Value::int(40));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sibling_interpreter_survives_other_interpreter_drop_collection() {
    // The registry is thread-local and shared across interpreters on the
    // thread: B's teardown collection trial-deletes A's live env and closures
    // too. A must keep working afterwards.
    let a = Interpreter::new();
    a.eval_str_compiled("(begin (define (helper-a x) (+ x 1)) (define (fa x) (helper-a x)))")
        .expect("define fa");
    {
        let b = Interpreter::new();
        b.eval_str_compiled("(begin (define (fb x) (* x 3)) (fb 2))")
            .expect("define fb");
    }
    assert_eq!(
        a.eval_str_compiled("(fa 41)").expect("call fa"),
        Value::int(42)
    );
}

// ── Macro-injected chunk consts ─────────────────────────────────────
//
// A macro can expand to a raw runtime VALUE (builtin or live closure);
// lower_expr's catch-all compiles it into chunk consts, so consts are real
// strong edges of the cycle graph. The tracer must report them exactly:
// undercounted, the const acts as a phantom external count pinning the
// closure's whole env graph; overcounted, a live closure gets severed.

#[test]
fn macro_injected_native_const_survives_collection() {
    let v = eval_ok(
        "(begin
           (defmacro inject () first)
           (define (user xs) ((inject) xs))
           (user (list 1 2 3))
           (gc/collect)
           (user (list 4 5 6)))",
    );
    assert_eq!(v, Value::int(4));
}

#[test]
fn macro_injected_closure_const_called_across_collections() {
    // Two evals: the macro body reads `f` at expansion time, so `f` must be
    // bound (executed) before the injecting program compiles. The injected
    // closure sits in `user`'s chunk consts and must stay callable across a
    // collection that traces it.
    let interp = Interpreter::new();
    interp
        .eval_str_compiled("(define (f x) (* x 3))")
        .expect("define f");
    let v = interp
        .eval_str_compiled(
            "(begin
               (defmacro inject-f () f)
               (define (user x) ((inject-f) x))
               (define before (user 5))
               (gc/collect)
               (list before (user 7)))",
        )
        .expect("macro-injected closure eval");
    assert_eq!(v, Value::list(vec![Value::int(15), Value::int(21)]));
}

// ── Interpreter teardown during panic unwind ───────────────────────

#[test]
fn interpreter_drop_during_panic_unwind_is_catchable() {
    // Interpreter::drop runs while this panic unwinds; it must not collect
    // there — any collector panic would be a panic-in-destructor during
    // cleanup, which aborts the process instead of unwinding to catch_unwind.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let interp = Interpreter::new();
        interp
            .eval_str_compiled("(define (f x) x)")
            .expect("define f");
        panic!("unwind through a live Interpreter");
    }));
    assert!(result.is_err(), "panic must unwind, not abort");
    // The skipped teardown leaves the dead env's candidates registered; the
    // next collection on this thread reclaims them and the runtime is intact.
    let interp = Interpreter::new();
    let v = interp
        .eval_str_compiled("(> (:collected (gc/collect)) 0)")
        .expect("post-panic eval");
    assert_eq!(v, Value::bool(true));
}

// ── Closure-free data cycles (M5: cold constructors register candidates) ──
//
// A cycle needs no closure: it can close entirely through the data cells
// (channel buffer, thunk `forced`, promise state, multimethod table). The
// constructors register `GcNode::{Channel,Thunk,Promise,MultiMethod}`
// candidates, so these cycles are discovered even when no closure candidate
// (and no live env binding) reaches them.

#[test]
fn data_only_channel_cycle_collected() {
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (mk)
             (let ((ch (channel/new 2)))
               (channel/send ch (list ch))
               nil))
           (mk)
           (> (:collected (gc/collect)) 0))",
    );
    assert_eq!(v, Value::bool(true));
}

#[test]
fn data_only_channel_cycle_live_survives_collect() {
    // Same self-buffering shape, still bound: the collection must keep the
    // whole cycle, and the buffered list must still yield the channel back.
    let v = eval_ok(
        "(begin
           (define ch (channel/new 2))
           (channel/send ch (list ch))
           (gc/collect)
           (channel? (first (channel/recv ch))))",
    );
    assert_eq!(v, Value::bool(true));
}

#[test]
fn data_only_delay_cycle_collected() {
    // The delay body reads a GLOBAL, so its wrapper closure has zero upvalues
    // (exempt from closure candidacy); forcing memoizes `t.forced = t`. Once
    // both globals are rebound, the self-cycle through the thunk's forced
    // cell is reachable only from the constructor-registered Thunk candidate.
    // Two evals: the first eval's VM pins the thunk through its global-load
    // inline cache (`(force t)` cached the loaded value — bounded mid-eval
    // retention that dies with the VM); the collect runs on a fresh VM.
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(
            "(begin
               (gc/collect)
               (define tmp nil)
               (define t (delay tmp))
               (set! tmp t)
               (force t)
               (set! tmp nil)
               (set! t nil))",
        )
        .expect("build garbage delay cycle");
    let v = interp
        .eval_str_compiled("(> (:collected (gc/collect)) 0)")
        .expect("collect");
    assert_eq!(v, Value::bool(true));
}

#[test]
fn data_only_delay_cycle_live_keeps_forcing_to_itself() {
    let v = eval_ok(
        "(begin
           (define tmp nil)
           (define t (delay tmp))
           (set! tmp t)
           (force t)
           (gc/collect)
           (= t (force t)))",
    );
    assert_eq!(v, Value::bool(true));
}

#[test]
fn data_only_promise_channel_cycle_collected() {
    // p resolves to ch and ch buffers p: a closure-free cycle through TWO
    // data cells (promise state + channel buffer). Locals die at return, so
    // only the Promise/Channel candidates reach it.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (mk)
             (let ((ch (channel/new 1)))
               (let ((p (async/resolved ch)))
                 (channel/send ch p)
                 nil)))
           (mk)
           (> (:collected (gc/collect)) 0))",
    );
    assert_eq!(v, Value::bool(true));
}

#[test]
fn data_only_promise_channel_cycle_live_round_trips() {
    let v = eval_ok(
        "(begin
           (define ch (channel/new 1))
           (define p (async/resolved ch))
           (channel/send ch p)
           (gc/collect)
           (define p-back (channel/recv ch))
           (channel? (await p-back)))",
    );
    assert_eq!(v, Value::bool(true));
}

#[test]
fn data_only_multimethod_self_cycle_collected() {
    // mm.methods[:self] = mm with a builtin dispatch fn: no closure anywhere
    // on the cycle. After the global rebind the MultiMethod candidate is the
    // only path to it. Two evals for the same inline-cache reason as
    // `data_only_delay_cycle_collected` (`defmethod` loads the global).
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(
            "(begin
               (gc/collect)
               (defmulti dead first)
               (defmethod dead :self dead)
               (set! dead 0))",
        )
        .expect("build garbage multimethod cycle");
    let v = interp
        .eval_str_compiled("(> (:collected (gc/collect)) 0)")
        .expect("collect");
    assert_eq!(v, Value::bool(true));
}

#[test]
fn data_only_multimethod_self_cycle_live_still_dispatches() {
    let v = eval_ok(
        "(begin
           (defmulti live first)
           (defmethod live :self live)
           (defmethod live :go (fn (xs) 42))
           (gc/collect)
           (live (list :go)))",
    );
    assert_eq!(v, Value::int(42));
}

// ── Data-birth registry-growth trigger (mid-eval, no closures) ─────
//
// The cold constructors register a candidate per allocation, and each dead
// candidate's `Weak` pins its allocation's RcBox until a pass prunes the
// entry. No closure is born inside these loops, so only `register_candidate`'s
// own threshold trigger can run a pass before the eval returns — without it,
// a single long eval retains O(total data births), not O(live).

#[test]
fn acyclic_data_churn_bounds_registry_mid_eval() {
    // 20k dead channels in one eval; the probe runs INSIDE the same eval,
    // before any outer safe point. Each threshold crossing takes the
    // prune-only fast path (the churn is acyclic), holding the registry to
    // roughly one growth-threshold batch.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (churn n)
             (if (<= n 0) nil
                 (begin (channel/new 1) (churn (- n 1)))))
           (churn 20000)
           (:registry-size (gc/stats)))",
    );
    let n = v.as_int().expect("registry size is an int");
    assert!(
        n <= 4096,
        "registry retained {n} entries after 20k acyclic channel births mid-eval"
    );
}

#[test]
fn cyclic_data_churn_collected_mid_eval() {
    // Self-buffering channels (closure-free cycles) stay LIVE registry
    // entries until traced, so pruning alone cannot bound this loop: the
    // birth-trigger pass must escalate to a full trace once live candidates
    // exceed half the threshold, severing the garbage cycles mid-eval. The
    // follow-up explicit collect reclaims only the post-last-pass tail —
    // far fewer than the total churned.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (churn n)
             (if (<= n 0) nil
                 (begin
                   (let ((ch (channel/new 2)))
                     (channel/send ch (list ch)))
                   (churn (- n 1)))))
           (churn 1500)
           (list (:registry-size (gc/stats)) (:collected (gc/collect))))",
    );
    let items = v.as_seq().expect("result list");
    let registry = items[0].as_int().expect("registry size");
    let tail = items[1].as_int().expect("collected");
    assert!(
        registry < 1300,
        "cyclic data churn left {registry} live registry entries mid-eval \
         (birth trigger never traced)"
    );
    assert!(
        (1..3000).contains(&tail),
        "explicit collect reclaimed {tail} nodes; expected only the \
         post-last-pass tail of the 1500 cycles"
    );
}

// ── Scheduler-idle safe point ───────────────────────────────────────

#[test]
fn scheduler_idle_safe_point_runs_pass_after_tasks_finish() {
    // A task churns 3000 dead channels: the data births cross the registry
    // threshold inside the task, so birth-trigger passes prune mid-task and
    // the registry is already bounded when the task finishes. The
    // scheduler-idle hook (after terminal-task reaping, task list empty)
    // remains the threshold-gated backstop for garbage released at reap
    // time and for passes that aborted mid-task. Asserted from the awaiting
    // eval, before any outer safe point runs — async-born churn must never
    // survive to the (gc/stats) read at O(total births).
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (spam n)
             (if (<= n 0) :done (begin (channel/new 1) (spam (- n 1)))))
           (define p (async/spawn (fn () (spam 3000))))
           (await p)
           (< (:registry-size (gc/stats)) 1024))",
    );
    assert_eq!(v, Value::bool(true));
}

// ── Zero-upvalue exemption (make_closure skips closure candidates) ─

#[test]
fn zero_upvalue_env_cycle_collected_via_env_candidate() {
    // `(define (f x) x)` compiles to a closure with ZERO upvalues, which
    // `make_closure` exempts from closure-candidate registration — the only
    // candidate reaching the resulting env⇄closure cycle (bindings → f →
    // `Closure.globals` → wrapper → same bindings) is the home-env WRAPPER
    // registered at adoption. Interpreter teardown runs the final unpinned
    // collection; if the exemption left the cycle without a covering
    // candidate, the whole global env would leak and this Weak would stay
    // live.
    let weak_bindings = {
        let interp = Interpreter::new();
        interp
            .eval_str_compiled("(define (f x) x)")
            .expect("define zero-upvalue closure");
        std::rc::Rc::downgrade(&interp.global_env.bindings)
    };
    assert_eq!(
        weak_bindings.strong_count(),
        0,
        "zero-upvalue env⇄closure cycle reclaimed via the env-wrapper candidate"
    );
}

#[test]
fn zero_upvalue_module_closure_bound_in_importer_env_collected_at_teardown() {
    // Cross-env variant: a zero-upvalue closure homed in a MODULE env is
    // bound into the importing (parent) env, so the cycle closes through the
    // ANCESTOR's bindings: importer.bindings → closure → module-env wrapper
    // → parent → importer wrapper → importer.bindings. Neither closure is a
    // registry candidate (both zero-upvalue); the cycle must be reached
    // through a registered home WRAPPER's parent chain.
    let dir = temp_dir("zero-uv-mod");
    let m = write_file(&dir, "mk.sema", "(define (make) (fn (x) (* x 3)))");
    let weak_bindings = {
        let interp = Interpreter::new();
        interp
            .eval_str_compiled(&format!(
                r#"(begin (import "{m}" make) (define g (make)) (g 4))"#
            ))
            .expect("import + bind module closure");
        std::rc::Rc::downgrade(&interp.global_env.bindings)
    };
    assert_eq!(
        weak_bindings.strong_count(),
        0,
        "cross-env cycle through the importer's bindings reclaimed at teardown"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Self-tail-call opt: no self-reference cycle per named-let entry ──

#[test]
fn self_tail_named_let_births_no_cycle() {
    // A self-tail-only named-let elides its self upvalue (issue #62), so each
    // loop entry creates no self-reference cycle for the collector to sever.
    // The pure counter loop captures nothing, so its closure is not even
    // registered as a candidate. After quiescing, running 300 such loops and
    // collecting must free nothing.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (churn i)
             (if (= i 0)
                 0
                 (begin
                   (let loop ((n 4)) (if (= n 0) n (loop (- n 1))))
                   (churn (- i 1)))))
           (define result (churn 300))
           (list result (:collected (gc/collect))))",
    );
    assert_eq!(v, Value::list(vec![Value::int(0), Value::int(0)]));
}

#[test]
fn self_tail_named_let_with_capture_still_collects_no_cycle() {
    // A self-tail-only loop that also captures an outer variable keeps that
    // upvalue (so its closure is registered), but eliding the self upvalue means
    // there is no cycle: repeated entries leave only acyclic garbage, so a
    // collection after quiescing severs nothing.
    let v = eval_ok(
        "(begin
           (gc/collect)
           (define (churn i)
             (if (= i 0)
                 :done
                 (begin
                   (let ((c i)) (let loop ((n 4) (acc 0)) (if (= n 0) (+ acc c) (loop (- n 1) (+ acc c)))))
                   (churn (- i 1)))))
           (define result (churn 300))
           (list result (:collected (gc/collect))))",
    );
    assert_eq!(v, Value::list(vec![Value::keyword("done"), Value::int(0)]));
}
