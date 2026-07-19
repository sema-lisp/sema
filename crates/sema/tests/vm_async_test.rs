mod common;

use std::cell::RefCell;

use common::eval;
use sema_core::runtime::{CancelReason, TaskOutcome};
use sema_core::{intern, Caps, NativeFn, Sandbox, SemaError, Value};
use sema_vm::runtime::{RootOptions, RootPoll};

thread_local! {
    static OWNER_COLLISION_INNER: RefCell<Option<sema_eval::Interpreter>> = const {
        RefCell::new(None)
    };
}

fn eval_vm_err(input: &str) -> String {
    let interp = sema_eval::Interpreter::new();
    interp.eval_str_compiled(input).unwrap_err().to_string()
}

// === Basic async/spawn + await ===

#[test]
fn async_spawn_await() {
    assert_eq!(
        eval(r#"(let ((p (async/spawn (fn () (+ 1 2))))) (async/await p))"#),
        Value::int(3)
    );
}

// === Regression: a task spawned before an outermost scheduler exit survives ===
//
// A task spawned in one top-level form, with an intervening `(async/all …)` on a
// DIFFERENT task (an outermost scheduler exit), then awaited in a later form, must
// still be alive when awaited. The adversarial-#7 reaping fix originally cleared
// ALL leftover tasks at the outermost exit, which wrongly killed `p` here and broke
// `examples/async-pipeline.sema` / `async-stress.sema` with "async/await: still
// pending after scheduler run". The reap is now terminal-only.
#[test]
fn task_survives_intervening_outermost_scheduler_exit() {
    assert_eq!(
        eval(
            r#"
            (define p (async/spawn (fn () (async/sleep 100) 42)))
            (async/all (list (async/spawn (fn () 1))))
            (await p)
            "#
        ),
        Value::int(42)
    );
}

// === async/map + async/spawn-all conveniences ===

#[test]
fn async_map_concurrent_in_order() {
    // Concurrent map, results in INPUT order regardless of completion order.
    assert_eq!(
        eval(r#"(async/map (fn (x) (* x x)) (list 1 2 3 4))"#),
        eval(r#"'(1 4 9 16)"#)
    );
}

#[test]
fn async_spawn_all_runs_thunks() {
    assert_eq!(
        eval(r#"(async/spawn-all (list (fn () :a) (fn () :b) (fn () :c)))"#),
        eval(r#"'(:a :b :c)"#)
    );
}

// === async special form ===

#[test]
fn async_special_form() {
    assert_eq!(
        eval(r#"(let ((p (async (+ 10 20)))) (await p))"#),
        Value::int(30)
    );
}

// === async with multiple expressions in body ===

#[test]
fn async_multi_body() {
    assert_eq!(
        eval(r#"(let ((p (async (define x 10) (define y 20) (+ x y)))) (await p))"#),
        Value::int(30)
    );
}

// === async/all ===

#[test]
fn async_all() {
    assert_eq!(
        eval(
            r#"(let ((p1 (async (+ 1 1))) (p2 (async (+ 2 2))) (p3 (async (+ 3 3)))) (async/all (list p1 p2 p3)))"#
        ),
        Value::list(vec![Value::int(2), Value::int(4), Value::int(6)])
    );
}

// === async/resolved and async/rejected ===

#[test]
fn async_resolved() {
    assert_eq!(eval("(async/await (async/resolved 42))"), Value::int(42));
}

#[test]
fn async_rejected() {
    // Awaiting a rejected promise re-raises the PRESERVED failure error verbatim
    // (the reason string), not a synthetic `task rejected:` wrapper. Plan: the
    // registry `TaskSettlement` carries the real `SemaError`; `async/await` maps
    // `Failed(err) -> Err(err)` without string-mangling.
    let err = eval_vm_err(r#"(async/await (async/rejected "oops"))"#);
    assert!(
        err.contains("oops"),
        "expected the preserved rejection cause, got: {err}"
    );
}

// === Promise predicates ===

#[test]
fn async_promise_predicate() {
    assert_eq!(
        eval("(async/promise? (async/resolved 1))"),
        Value::bool(true)
    );
}

#[test]
fn async_promise_predicate_false() {
    assert_eq!(eval("(async/promise? 42)"), Value::bool(false));
}

#[test]
fn async_resolved_predicate() {
    assert_eq!(
        eval("(async/resolved? (async/resolved 1))"),
        Value::bool(true)
    );
}

#[test]
fn async_rejected_predicate() {
    assert_eq!(
        eval(r#"(async/rejected? (async/rejected "x"))"#),
        Value::bool(true)
    );
}

#[test]
fn async_pending_predicate() {
    // A freshly spawned task is pending until awaited
    assert_eq!(eval("(async/pending? (async (+ 1 2)))"), Value::bool(true));
}

// === Channel basics ===

#[test]
fn channel_send_recv() {
    assert_eq!(
        eval("(let ((ch (channel/new 3))) (channel/send ch 10) (channel/send ch 20) (channel/recv ch))"),
        Value::int(10)
    );
}

#[test]
fn channel_fifo() {
    assert_eq!(
        eval("(let ((ch (channel/new 3))) (channel/send ch :a) (channel/send ch :b) (channel/recv ch) (channel/recv ch))"),
        Value::keyword("b")
    );
}

#[test]
fn channel_count() {
    assert_eq!(
        eval("(let ((ch (channel/new 5))) (channel/send ch 1) (channel/send ch 2) (channel/count ch))"),
        Value::int(2)
    );
}

#[test]
fn channel_empty() {
    assert_eq!(eval("(channel/empty? (channel/new 1))"), Value::bool(true));
}

#[test]
fn channel_predicate() {
    assert_eq!(eval("(channel? (channel/new 1))"), Value::bool(true));
}

#[test]
fn channel_predicate_false() {
    assert_eq!(eval("(channel? 42)"), Value::bool(false));
}

#[test]
fn channel_close() {
    assert_eq!(
        eval("(let ((ch (channel/new 1))) (channel/close ch) (channel/closed? ch))"),
        Value::bool(true)
    );
}

#[test]
fn channel_try_recv_empty() {
    assert_eq!(eval("(channel/try-recv (channel/new 1))"), Value::nil());
}

#[test]
fn channel_full() {
    assert_eq!(
        eval("(let ((ch (channel/new 1))) (channel/send ch 42) (channel/full? ch))"),
        Value::bool(true)
    );
}

// === Async producer/consumer with channels ===

#[test]
fn async_producer_consumer() {
    assert_eq!(
        eval(
            r#"(let ((ch (channel/new 1)))
          (let ((producer (async (channel/send ch 42)))
                (consumer (async (channel/recv ch))))
            (await consumer)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn async_producer_consumer_multi() {
    assert_eq!(
        eval(
            r#"(let ((ch (channel/new 2)))
          (let ((producer (async
                  (channel/send ch 10)
                  (channel/send ch 20)))
                (consumer (async
                  (let ((a (channel/recv ch))
                        (b (channel/recv ch)))
                    (+ a b)))))
            (await consumer)))"#
        ),
        Value::int(30)
    );
}

// === async/race ===

#[test]
fn async_race_first_wins() {
    assert_eq!(
        eval(
            r#"(let ((fast (async/resolved 1))
              (slow (async (+ 2 2))))
          (async/race (list fast slow)))"#
        ),
        Value::int(1)
    );
}

#[test]
fn async_race_returns_first_resolved_in_list_order() {
    // With real race semantics, the sender settles first after sending. The
    // blocked receiver is only woken on a later scheduler turn.
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (let ((first (async (channel/recv ch)))
                    (second (async
                              (channel/send ch :sent)
                              :sender-done)))
                (async/race (list first second))))
        "#
        ),
        Value::keyword("sender-done")
    );
}

// === async/sleep ===

#[test]
fn async_sleep_returns_nil() {
    assert_eq!(
        eval("(let ((p (async (async/sleep 0)))) (await p))"),
        Value::nil()
    );
}

#[test]
fn sleep_duration_determines_wake_order_not_spawn_order() {
    // Three tasks spawned in the order c, a, b but sleeping 30/10/20ms. The
    // virtual clock must wake them in *duration* order (a, b, c) regardless of
    // spawn order — the drained channel proves it. This is deterministic on
    // every platform (and instant in WASM, where the virtual clock advances
    // without real waits).
    let out = eval(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list (async (async/sleep 30) (channel/send out :c))
                  (async (async/sleep 10) (channel/send out :a))
                  (async (async/sleep 20) (channel/send out :b))))
          (list (channel/recv out) (channel/recv out) (channel/recv out)))
    "#,
    );
    assert_eq!(
        out,
        Value::list(vec![
            Value::keyword("a"),
            Value::keyword("b"),
            Value::keyword("c"),
        ])
    );
}

#[test]
fn equal_sleeps_wake_in_spawn_order() {
    // Equal durations fall back to deterministic spawn (FIFO) order.
    let out = eval(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list (async (async/sleep 5) (channel/send out 1))
                  (async (async/sleep 5) (channel/send out 2))
                  (async (async/sleep 5) (channel/send out 3))))
          (list (channel/recv out) (channel/recv out) (channel/recv out)))
    "#,
    );
    assert_eq!(
        out,
        Value::list(vec![Value::int(1), Value::int(2), Value::int(3)])
    );
}

#[test]
fn step_limit_aborts_runaway_loop() {
    // The loop guard (revived from the dead eval_step_limit) must abort an
    // infinite loop instead of hanging. This is what protects the playground
    // main thread from freezing.
    let interp = sema_eval::Interpreter::new();
    interp.ctx.set_eval_step_limit(200_000);
    let err = interp
        .eval_str_compiled("(let loop () (loop))")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("step limit"),
        "expected step-limit abort, got: {err}"
    );
}

fn always_cancel() -> bool {
    true
}

#[test]
fn interrupt_callback_cancels_evaluation() {
    // A registered interrupt callback (the playground Stop button installs one
    // backed by a shared cancel flag) must abort a running loop.
    sema_core::set_interrupt_callback(always_cancel);
    let interp = sema_eval::Interpreter::new();
    let result = interp.eval_str_compiled("(let loop () (loop))");
    sema_core::clear_interrupt_callback();
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("cancelled"),
        "expected cancellation, got: {err}"
    );
}

#[test]
fn concurrent_sleeps_resolve_in_duration_order() {
    // Three sibling tasks sleep 30/10/20 ms then send :c/:a/:b. Sleeps are
    // cooperative and duration-ordered, so the shortest sleeper wakes first
    // regardless of spawn order: the received order is a,b,c (10 < 20 < 30).
    let out = eval(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list (async (async/sleep 30) (channel/send out :c))
                  (async (async/sleep 10) (channel/send out :a))
                  (async (async/sleep 20) (channel/send out :b))))
          (list (channel/recv out) (channel/recv out) (channel/recv out)))
    "#,
    );

    assert_eq!(
        out,
        Value::list(vec![
            Value::keyword("a"),
            Value::keyword("b"),
            Value::keyword("c"),
        ]),
        "cooperative sleeps must resolve in duration order regardless of spawn order"
    );
}

// === Error cases ===

#[test]
fn channel_send_closed_error() {
    let err = eval_vm_err("(let ((ch (channel/new 1))) (channel/close ch) (channel/send ch 1))");
    assert!(err.contains("closed"), "expected closed error, got: {err}");
}

#[test]
fn channel_close_with_blocked_sender_reports_lost_value() {
    // Regression for bug C3: closing a channel under a blocked sender silently
    // dropped the pending value. The error should clearly indicate the send
    // was pending and name the lost value.
    let err = eval_vm_err(
        "(let ((ch (channel/new 1))) \
           (channel/send ch 1) \
           (let ((p (async (channel/send ch 2)))) \
             (channel/close ch) \
             (await p)))",
    );
    assert!(
        err.contains("closed") && (err.contains("send was pending") || err.contains("2")),
        "expected pending-send closed error mentioning the lost value, got: {err}"
    );
}

#[test]
fn channel_recv_empty_error() {
    let err = eval_vm_err("(channel/recv (channel/new 1))");
    assert!(err.contains("empty"), "expected empty error, got: {err}");
}

#[test]
fn channel_send_full_error() {
    let err = eval_vm_err("(let ((ch (channel/new 1))) (channel/send ch 1) (channel/send ch 2))");
    assert!(err.contains("full"), "expected full error, got: {err}");
}

#[test]
fn channel_zero_capacity_error() {
    let err = eval_vm_err("(channel/new 0)");
    assert!(
        err.contains("capacity"),
        "expected capacity error, got: {err}"
    );
}

// === Async is accepted on the (sole) VM backend ===

#[test]
fn default_backend_accepts_async() {
    // Every eval entry point runs on the VM, so async/await is accepted
    // via the default `eval_str` path.
    let interp = sema_eval::Interpreter::new();
    let result = interp
        .eval_str("(await (async (+ 1 2)))")
        .expect("async must work on the VM backend");
    assert_eq!(result, sema_core::Value::int(3));
}

// ── Nested async ──────────────────────────────────────────────────

#[test]
fn nested_async_await() {
    assert_eq!(eval("(await (async (await (async 7))))"), Value::int(7),);
}

#[test]
fn nested_async_multiple_awaits() {
    assert_eq!(
        eval("(await (async (+ (await (async 3)) (await (async 4)))))"),
        Value::int(7),
    );
}

#[test]
fn triple_nested_async() {
    assert_eq!(
        eval("(await (async (await (async (await (async 42))))))"),
        Value::int(42),
    );
}

// === Bug regression tests ===

// `await_rejected_propagates` was removed: it is fully subsumed by
// `await_rejected_propagates_division_error` below, which asserts the actual
// inner-cause substring rather than just non-empty error text.

#[test]
fn async_context_preserved_after_nested_run() {
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (await (async
                (async/run)
                (channel/send ch 42)
                (channel/recv ch))))
        "#
        ),
        Value::int(42)
    );
}

#[test]
fn channel_close_rejects_pending_send() {
    let err = eval_vm_err(
        r#"
        (let ((ch (channel/new 1)))
          (channel/send ch :fill)
          (let ((sender (async (channel/send ch :blocked))))
            (channel/close ch)
            (channel/recv ch)
            (await sender)))
    "#,
    );
    assert!(
        err.contains("closed") || err.contains("rejected"),
        "should reject pending send on closed channel, got: {err}"
    );
}

#[test]
fn nested_async_with_channel() {
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (await (async
                (let ((inner (async (channel/recv ch))))
                  (channel/send ch 99)
                  (await inner)))))
        "#
        ),
        Value::int(99),
    );
}

// === async/timeout ===

#[test]
fn timeout_resolved_in_time() {
    assert_eq!(
        eval("(async/timeout 1000 (async/resolved 42))"),
        Value::int(42),
    );
}

#[test]
fn timeout_task_completes_in_time() {
    assert_eq!(eval("(async/timeout 1000 (async (+ 1 2)))"), Value::int(3),);
}

#[test]
fn timeout_already_rejected() {
    // An already-rejected promise wins the timeout race; its PRESERVED failure
    // cause surfaces (not a `task rejected:` wrapper).
    let err = eval_vm_err(r#"(async/timeout 1000 (async/rejected "oops"))"#);
    assert!(
        err.contains("oops"),
        "expected the preserved rejection cause, got: {err}"
    );
}

#[test]
fn timeout_negative_duration_error() {
    let err = eval_vm_err("(async/timeout -1 (async/resolved 1))");
    assert!(
        err.contains("non-negative"),
        "expected non-negative error, got: {err}"
    );
}

#[test]
fn timeout_expires() {
    let err = eval_vm_err(
        r#"
        (let ((ch (channel/new 1)))
          (async/timeout 50 (async (channel/recv ch))))
    "#,
    );
    assert!(err.contains("timed out"), "expected timeout, got: {err}");
}

#[test]
fn timeout_beats_sleeping_task() {
    let err = eval_vm_err("(async/timeout 10 (async (async/sleep 100) 42))");
    assert!(
        err.contains("timed out"),
        "expected timeout before sleep completion, got: {err}"
    );
}

#[test]
fn timeout_zero_lets_ready_work_complete() {
    // A 0 ms timeout must still let synchronously-ready work finish — it only
    // trips once the virtual clock actually reaches the deadline with the task
    // still pending. (Regression guard: the deadline used to be checked before
    // the ready task ran, so this timed out instead of returning the value.)
    assert_eq!(eval("(async/timeout 0 (async 42))"), Value::int(42));
    assert_eq!(eval("(async/timeout 1 (async (+ 20 22)))"), Value::int(42));
}

#[test]
fn timeout_zero_still_expires_on_blocking_work() {
    // ...but a 0 ms timeout on work that genuinely blocks still expires.
    let err = eval_vm_err("(async/timeout 0 (async (channel/recv (channel/new 1))))");
    assert!(err.contains("timed out"), "expected timeout, got: {err}");
}

#[test]
fn sleep_duration_is_capped() {
    // An out-of-range sleep is rejected up front; otherwise the native virtual
    // clock would wait the whole delta in one multi-year thread::sleep (and the
    // logical clock could overflow).
    let err = eval_vm_err("(await (async (async/sleep 9223372036854775807) 1))");
    assert!(
        err.contains("exceeds maximum"),
        "expected sleep cap error, got: {err}"
    );
}

#[test]
fn async_race_returns_first_to_settle_not_list_order() {
    assert_eq!(
        eval("(async/race (list (async (async/sleep 100) :slow) (async :fast)))"),
        Value::keyword("fast"),
    );
}

#[test]
fn async_all_ignores_unrelated_blocked_task() {
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (let ((bg (async (channel/recv ch)))
                    (p (async 1)))
                (async/all (list p))))
        "#
        ),
        Value::list(vec![Value::int(1)]),
    );
}

#[test]
fn async_task_survives_separate_vm_evals() {
    let interp = sema_eval::Interpreter::new();
    interp
        .eval_str_compiled("(define p (async (async/sleep 1) 42))")
        .unwrap();
    assert_eq!(
        interp.eval_str_compiled("(await p)").unwrap(),
        Value::int(42)
    );
}

#[test]
fn serial_list_respects_sandbox() {
    let sandbox = Sandbox::deny(Caps::SERIAL);
    let interp = sema_eval::Interpreter::new_with_sandbox(&sandbox);
    let err = interp
        .eval_str_compiled("(serial/list)")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Permission denied") && err.contains("serial"),
        "expected serial sandbox denial, got: {err}"
    );
}

// === Task cancellation ===

#[test]
fn cancel_pending_task() {
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (let ((p (async (channel/recv ch))))
                (async/cancel p)
                (async/cancelled? p)))
        "#
        ),
        Value::bool(true),
    );
}

#[test]
fn cancel_awaited_task_rejects() {
    let err = eval_vm_err(
        r#"
        (let ((ch (channel/new 1)))
          (let ((p (async (channel/recv ch))))
            (async/cancel p)
            (await p)))
    "#,
    );
    assert!(
        err.contains("cancelled"),
        "expected cancellation error, got: {err}"
    );
}

#[test]
fn cancel_completed_task_is_noop() {
    assert_eq!(
        eval(
            r#"
            (let ((p (async 42)))
              (await p)
              (async/cancel p)
              (async/resolved? p))
        "#
        ),
        Value::bool(true),
    );
}

// === Observational combinator short-circuiting ===

#[test]
fn async_all_failure_does_not_cancel_supplied_sibling() {
    assert_eq!(
        eval(
            r#"
            (define slow (async (async/sleep 10) :slow-finished))
            (define boom (async (error "boom")))
            (try (async/all (list boom slow)) (catch e nil))
            (list (async/cancelled? slow) (await slow))
            "#,
        ),
        Value::list(vec![Value::bool(false), Value::keyword("slow-finished"),]),
    );
}

#[test]
fn async_race_does_not_cancel_supplied_loser() {
    assert_eq!(
        eval(
            r#"
            (define slow (async (async/sleep 10) :slow-finished))
            (define fast (async :fast))
            (define result (async/race (list slow fast)))
            (list result (async/cancelled? slow) (await slow))
            "#,
        ),
        Value::list(vec![
            Value::keyword("fast"),
            Value::bool(false),
            Value::keyword("slow-finished"),
        ]),
    );
}

// Short-circuiting an observation must not affect unrelated in-flight work.
#[test]
fn combinator_short_circuit_spares_unrelated_task() {
    assert_eq!(
        eval(
            r#"
            (define bg (async (async/sleep 50) 99))
            (define slow (async (async/sleep 1000) :slow))
            (define fast (async :fast))
            (async/race (list slow fast))
            (await bg)
            "#
        ),
        Value::int(99),
    );
}

// === Bug regression: yield signal through op::CALL ===

#[test]
fn channel_recv_via_local_variable_yields_correctly() {
    // channel/recv called through a local variable binding goes through
    // op::CALL (not CALL_NATIVE or CALL_GLOBAL). The yield signal must
    // still be checked after call_value returns.
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1))
                  (recv channel/recv))
              (let ((producer (async (channel/send ch 42)))
                    (consumer (async (recv ch))))
                (await consumer)))
        "#
        ),
        Value::int(42),
    );
}

#[test]
fn channel_send_via_local_variable_yields_correctly() {
    // Same bug but for channel/send through a local binding
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1))
                  (send channel/send))
              (let ((consumer (async (channel/recv ch))))
                (send ch 99)
                (await consumer)))
        "#
        ),
        Value::int(99),
    );
}

// === Bug regression: false deadlock with mixed blocked tasks ===

#[test]
fn sleeping_task_unblocks_channel_recv() {
    // A sleeping task will eventually send to a channel. The scheduler
    // must not report deadlock when one task sleeps and another waits
    // on channel/recv.
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (async/spawn (fn () (async/sleep 1) (channel/send ch 42)))
              (let ((consumer (async (channel/recv ch))))
                (await consumer)))
        "#
        ),
        Value::int(42),
    );
}

// === async/all error handling ===

#[test]
fn async_all_rejects_on_any_failure() {
    let err = eval_vm_err(r#"(async/all (list (async 1) (async (/ 1 0)) (async 3)))"#);
    assert!(
        err.contains("rejected") || err.contains("division") || err.contains("zero"),
        "expected rejection from division error, got: {err}"
    );
}

// ASYNC-3 companion: async/all must surface the first settled rejection even when
// an earlier list entry remains pending. This test observes only error selection;
// it intentionally makes no ownership or cancellation claim about the sibling.
#[test]
fn async_all_surfaces_first_settled_rejection() {
    let err = eval_vm_err(
        r#"
        (async/all (list (async (async/sleep 1000) (error "later"))
                         (async (error "first"))))
        "#,
    );
    assert!(
        err.contains("first"),
        "async/all must report the first settled rejection; got: {err}"
    );
}

#[test]
fn async_all_empty_list() {
    assert_eq!(eval("(async/all (list))"), Value::list(vec![]));
}

// === async/race edge cases ===

#[test]
fn async_race_empty_list_errors() {
    let err = eval_vm_err("(async/race (list))");
    assert!(
        err.contains("requires at least one"),
        "expected arity error, got: {err}"
    );
}

#[test]
fn async_race_all_rejected() {
    let err = eval_vm_err(r#"(async/race (list (async (error "a")) (async (error "b"))))"#);
    assert!(
        err.contains("\"a\"")
            || err.contains("\"b\"")
            || err.contains(": a")
            || err.contains(": b"),
        "expected rejection error mentioning \"a\" or \"b\", got: {err}"
    );
}

// === Channel close semantics ===

#[test]
fn channel_recv_closed_empty_returns_nil() {
    assert_eq!(
        eval("(let ((ch (channel/new 1))) (channel/close ch) (channel/recv ch))"),
        Value::nil(),
    );
}

#[test]
fn channel_recv_wakes_on_close_with_nil() {
    // An async task blocked on recv should be woken with nil when
    // the channel is closed.
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (let ((consumer (async (channel/recv ch))))
                (channel/close ch)
                (await consumer)))
        "#
        ),
        Value::nil(),
    );
}

// === Timeout edge cases ===

#[test]
fn timeout_zero_expires_immediately() {
    let err = eval_vm_err(r#"(async/timeout 0 (async (channel/recv (channel/new 1))))"#);
    assert!(
        err.contains("timed out"),
        "expected timeout error, got: {err}"
    );
}

// === Deadlock detection ===

#[test]
fn deadlock_detected_two_tasks_waiting() {
    let err = eval_vm_err(
        r#"
        (let ((ch1 (channel/new 1))
              (ch2 (channel/new 1)))
          (let ((t1 (async (channel/recv ch1)))
                (t2 (async (channel/recv ch2))))
            (async/all (list t1 t2))))
    "#,
    );
    assert!(
        err.contains("deadlock") || err.contains("blocked"),
        "expected deadlock error, got: {err}"
    );
}

// === Strengthen weak assertion ===

#[test]
fn await_rejected_propagates_division_error() {
    let err = eval_vm_err(r#"(await (async (await (async (/ 1 0)))))"#);
    assert!(
        err.contains("division") || err.contains("zero"),
        "should propagate division-by-zero cause, got: {err}"
    );
}

// === Multiple senders ===

// === Mutation-testing-derived coverage ===

#[test]
fn channel_buffered_send_with_room() {
    // Exercises buf.len() < capacity when buffer is partially full.
    // Mutation testing found this path was untested (< vs == survived).
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 3)))
              (channel/send ch 1)
              (channel/send ch 2)
              (channel/send ch 3)
              (+ (channel/recv ch) (channel/recv ch) (channel/recv ch)))
        "#
        ),
        Value::int(6),
    );
}

#[test]
fn cancel_already_failed_task_is_noop() {
    // Mutation testing found the cancel guard (Done || Failed) was
    // not fully tested — only Done was tested, not Failed.
    let err = eval_vm_err(
        r#"
        (let ((p (async (/ 1 0))))
          (await p))
    "#,
    );
    assert!(
        err.contains("division") || err.contains("zero"),
        "got: {err}"
    );
}

#[test]
fn channel_two_senders_one_receiver() {
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 2)))
              (let ((s1 (async (channel/send ch 10)))
                    (s2 (async (channel/send ch 20)))
                    (r  (async (+ (channel/recv ch) (channel/recv ch)))))
                (await r)))
        "#
        ),
        Value::int(30),
    );
}

/// Task 0c-7: two senders queue (in that order) on a full capacity-1 channel
/// before any receiver exists; a receiver then drains twice. The in-place
/// handoff must still ask the channel registry — which enforces FIFO via its
/// own sender queue — rather than resolving whichever sender happens to be
/// nearest: a send while ANOTHER sender is already queued must queue BEHIND
/// it, never hand off ahead of it, even once a receiver is waiting.
#[test]
fn channel_fifo_preserved_under_immediate_handoff() {
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (let ((s1 (async (channel/send ch :first)))
                    (s2 (async (channel/send ch :second)))
                    (r  (async (list (channel/recv ch) (channel/recv ch)))))
                (await r)))
        "#
        ),
        Value::list(vec![Value::keyword("first"), Value::keyword("second")]),
    );
}

// === Async ops inside higher-order stdlib callbacks ===
//
// HOFs (for-each, map, filter, foldl, sort-by, ...) invoke VM closures
// through the closure's NativeFn fallback path. That fallback creates a
// fresh VM, so any async yield inside the callback used to fail with
// "async yield outside of scheduler context". Now resolved by spawning
// the callback as a real task and awaiting it inline.

#[test]
fn for_each_callback_can_yield_on_full_channel() {
    // Producer's for-each tries to send 5 values into a capacity-3 channel.
    // The 4th send must yield (buffer full) and resume after the consumer
    // drains. Sum should be 1+2+3+4+5 = 15.
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 3)))
              (let ((producer (async
                                (for-each (fn (n) (channel/send ch n))
                                          (list 1 2 3 4 5))
                                (channel/close ch)))
                    (consumer (async
                                (let loop ((sum 0))
                                  (let ((v (channel/recv ch)))
                                    (if (nil? v) sum (loop (+ sum v))))))))
                (await consumer)))
        "#
        ),
        Value::int(15),
    );
}

#[test]
fn native_callback_passed_directly_suspends_cooperatively() {
    // A suspending native fn (channel/recv) passed DIRECTLY as a HOF callback
    // now suspends structurally through the `NativeOutcome` ABI — the runtime
    // drives the map's callback cooperatively, so each `channel/recv` parks and
    // resumes correctly instead of dropping the yield. Draining a 1/2/closed
    // channel across three recvs yields the two values then the closed sentinel
    // (nil). (This retires the old "wrap it in a lambda" limitation, which was a
    // consequence of the legacy yield-signal not propagating through the HOF's
    // Rust loop.)
    assert_eq!(
        eval(
            r#"
        (let ((ch (channel/new 1)))
          (let ((producer (async
                            (channel/send ch 1)
                            (channel/send ch 2)
                            (channel/close ch)))
                (consumer (async (map channel/recv (list ch ch ch)))))
            (await consumer)))
        "#,
        ),
        Value::list(vec![Value::int(1), Value::int(2), Value::nil()]),
    );
}

#[test]
fn map_callback_can_await_promise() {
    // map's callback awaits a per-item promise. All items should resolve.
    assert_eq!(
        eval(
            r#"
            (let ((p (async
                       (map (fn (n) (await (async (* n n))))
                            (list 2 3 4)))))
              (await p))
        "#
        ),
        Value::list(vec![Value::int(4), Value::int(9), Value::int(16)]),
    );
}

// Regression: nested await on a rejected promise re-raises the ORIGINAL cause
// once, never nesting synthetic wrappers (A2). Under the registry model the
// failure `SemaError` is preserved through every await hop, so awaiting a
// promise whose task itself awaited a rejected promise still surfaces the bare
// "boom" cause — no accumulating `task rejected:` prefixes.
#[test]
fn nested_await_rejection_does_not_double_prefix() {
    let err = eval_vm_err(
        r#"
        (let ((inner (async/rejected "boom")))
          (let ((outer (async (await inner))))
            (await outer)))
        "#,
    );
    assert!(
        err.contains("boom"),
        "expected the preserved rejection cause, got: {err}"
    );
    assert!(
        !err.contains("task rejected"),
        "the preserved cause must not be re-wrapped as 'task rejected': {err}"
    );
}

// Regression: async/timeout rejects unreasonably large durations (A3).
#[test]
fn async_timeout_rejects_huge_duration() {
    let err = eval_vm_err(r#"(async/timeout 9999999999999 (async 1))"#);
    assert!(
        err.contains("exceeds maximum"),
        "expected 'exceeds maximum' error, got: {err}"
    );
}

// === Async semantics pass: A1 + A4 + D2 ===========================

// A1: scheduler picks ready tasks in spawn order, not swap-remove order.
#[test]
fn scheduler_picks_ready_tasks_in_spawn_order() {
    // Three sequential channel sends followed by three receives on a
    // capacity-1 channel. Before A1 (swap_remove): (1 3 2). Now: (1 2 3).
    assert_eq!(
        eval(
            r#"
            (let ((ch (channel/new 1)))
              (let ((s1 (async (channel/send ch 1)))
                    (s2 (async (channel/send ch 2)))
                    (s3 (async (channel/send ch 3)))
                    (r  (async (list (channel/recv ch)
                                     (channel/recv ch)
                                     (channel/recv ch)))))
                (await r)))
        "#
        ),
        Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
    );
}

// A4: async/cancel returns a boolean.
#[test]
fn async_cancel_returns_true_when_transitioning_pending() {
    assert_eq!(
        eval(r#"(let ((p (async (async/sleep 100)))) (async/cancel p))"#),
        Value::bool(true),
    );
}

#[test]
fn async_cancel_returns_false_for_never_spawned_promise() {
    assert_eq!(
        eval(r#"(async/cancel (async/resolved 42))"#),
        Value::bool(false),
    );
    assert_eq!(
        eval(r#"(async/cancel (async/rejected "x"))"#),
        Value::bool(false),
    );
}

#[test]
fn async_cancel_returns_false_for_already_resolved_spawn() {
    assert_eq!(
        eval(r#"(let ((p (async 42))) (await p) (async/cancel p))"#),
        Value::bool(false),
    );
}

#[test]
fn async_cancel_returns_false_on_double_cancel() {
    assert_eq!(
        eval(
            r#"(let ((p (async (async/sleep 100))))
                 (async/cancel p)
                 (async/cancel p))"#
        ),
        Value::bool(false),
    );
}

// D2: PromiseState::Cancelled is a peer variant, not a magic string.
#[test]
fn async_cancelled_is_distinct_from_rejected_with_same_string() {
    // (async/rejected "cancelled") no longer fools async/cancelled?.
    assert_eq!(
        eval(r#"(async/cancelled? (async/rejected "cancelled"))"#),
        Value::bool(false),
    );
    // It IS a real rejection though.
    assert_eq!(
        eval(r#"(async/rejected? (async/rejected "cancelled"))"#),
        Value::bool(true),
    );
}

#[test]
fn cancelled_promise_classifies_correctly() {
    // Cancelled is neither resolved nor rejected nor pending — they partition.
    assert_eq!(
        eval(
            r#"
            (let ((p (async (async/sleep 100))))
              (async/cancel p)
              (list (async/cancelled? p)
                    (async/rejected? p)
                    (async/resolved? p)
                    (async/pending? p)))
        "#
        ),
        Value::list(vec![
            Value::bool(true),
            Value::bool(false),
            Value::bool(false),
            Value::bool(false),
        ]),
    );
}

#[test]
fn awaiting_cancelled_promise_reports_cancellation_distinctly() {
    let err = eval_vm_err(
        r#"(let ((p (async (async/sleep 100))))
             (async/cancel p)
             (await p))"#,
    );
    assert!(
        err.contains("cancelled"),
        "expected cancellation in error, got: {err}"
    );
    assert!(
        !err.contains("task rejected"),
        "cancellation should NOT surface as 'task rejected': {err}"
    );
}

// === parallel / pipeline — bounded fan-out combinators (workflow.js semantics) ===

// pipeline preserves INPUT order even though completion order is reversed (leaf i
// sleeps (8-i)*5ms, so item 7 finishes first, item 0 last).
#[test]
fn pipeline_preserves_input_order() {
    assert_eq!(
        eval(
            r#"(pipeline (list 0 1 2 3 4 5 6 7)
                  (fn (i) (begin (async/sleep (* (- 8 i) 5)) i)))"#
        ),
        common::eval(r#"'(0 1 2 3 4 5 6 7)"#)
    );
}

// pipeline threads each item through MULTIPLE stages; a stage that throws drops THAT
// item to nil (skipping its remaining stages) without aborting the batch.
#[test]
fn pipeline_stages_and_nil_on_failure() {
    assert_eq!(
        eval(
            r#"(pipeline (list 0 1 2)
                  (fn (i) (if (= i 1) (throw "boom") i))
                  (fn (x) (* x 10)))"#
        ),
        // nil must be the real nil value, so build the expected list unquoted.
        common::eval(r#"(list 0 nil 20)"#)
    );
}

// parallel runs a list of zero-arg thunks concurrently (barrier), results in input
// order; a throwing thunk yields nil rather than aborting the batch.
#[test]
fn parallel_runs_thunks_nil_on_failure() {
    assert_eq!(
        eval(r#"(parallel (list (fn () 1) (fn () (throw "boom")) (fn () 3)))"#),
        common::eval(r#"(list 1 nil 3)"#)
    );
}

// Degenerate inputs must not deadlock/panic the semaphore/await plumbing: an empty list
// yields an empty list; pipeline with ZERO stages is identity passthrough.
#[test]
fn parallel_and_pipeline_handle_empty_and_zero_stages() {
    assert_eq!(eval(r#"(parallel (list))"#), common::eval(r#"'()"#));
    assert_eq!(
        eval(r#"(pipeline (list) (fn (x) x))"#),
        common::eval(r#"'()"#)
    );
    assert_eq!(
        eval(r#"(pipeline (list 1 2 3))"#),
        common::eval(r#"'(1 2 3)"#)
    );
}

// === WP-TIMING: `sleep` / `retry` yield instead of blocking the VM thread ===
//
// `sleep_builtin_...`'s proof uses the same completion-order oracle as
// `sleep_duration_determines_wake_order_not_spawn_order` above: a sibling
// with no wait of its own sends to a channel BEFORE the sleeper can, which is
// only possible if the wait parks the task (yields) rather than blocking the
// single VM thread outright. Under the pre-fix `sleep` (unconditional
// `thread::sleep`) the sleeping task would run start-to-finish in one
// uninterrupted scheduler step, so the sibling could never get in first and
// the order would come out reversed.
//
// `retry_backoff_...` can't reuse that same no-wait-sibling shape: calling a
// Sema lambda thunk from a native while `in_async_context()` ALREADY routes
// through `run_closure_as_inline_task` (vm.rs's `make_closure` NativeFn
// wrapper) regardless of this WP, so a sibling with no wait of its own would
// get a free ride during retry's very first (pre-backoff) attempt and the
// test would pass even against the unfixed code — not a real proof. Instead
// the sibling races retry's backoff on the VIRTUAL CLOCK: it does its own
// short `async/sleep`, shorter than retry's backoff delay. If the backoff is
// a real `YieldReason::Sleep` (the fix), it competes fairly in the same wake
// order as any other sleeper and the shorter sibling sleep must resolve
// first (exactly like `sleep_duration_determines_wake_order_not_spawn_
// order`). If the backoff is a raw blocking `thread::sleep` (the bug), it
// runs on the OS thread with the scheduler's virtual clock frozen — the
// sibling's sleep can't be woken until that native call returns, so retry
// finishes (and sends first) before the sibling ever wakes.
#[test]
fn sleep_builtin_yields_lets_sibling_complete_first() {
    let out = eval(
        r#"
        (let ((out (channel/new 8)))
          (async/all
            (list (async/spawn (fn () (sleep 30) (channel/send out :slow)))
                  (async/spawn (fn () (channel/send out :fast)))))
          (list (channel/recv out) (channel/recv out)))
        "#,
    );
    assert_eq!(
        out,
        Value::list(vec![Value::keyword("fast"), Value::keyword("slow")]),
        "a sibling with no sleep must complete before the sleeper wakes"
    );
}

#[test]
fn retry_backoff_yields_lets_sibling_complete_first() {
    // The thunk fails once (counter 0 -> 1, still < 2) so `retry` backs off
    // 40ms before its second attempt succeeds (counter -> 2); the sibling's
    // own sleep (10ms) is strictly shorter, so it must win the wake race.
    let out = eval(
        r#"
        (let ((out (channel/new 8))
              (counter 0))
          (async/all
            (list (async/spawn (fn ()
                    (retry (fn ()
                             (set! counter (+ counter 1))
                             (if (< counter 2) (error "not yet") counter))
                           {:max-attempts 5 :base-delay-ms 40})
                    (channel/send out :slow)))
                  (async/spawn (fn () (async/sleep 10) (channel/send out :fast)))))
          (list (channel/recv out) (channel/recv out)))
        "#,
    );
    assert_eq!(
        out,
        Value::list(vec![Value::keyword("fast"), Value::keyword("slow")]),
        "a sibling sleeping for LESS than retry's backoff delay must wake first"
    );
}

#[test]
fn retry_backoff_in_async_context_still_succeeds_and_returns_value() {
    // Full functional check alongside the ordering proof above: exhausting
    // two failures then succeeding on the third attempt returns the success
    // value, same as the sync-path oracle (`test_retry_counter`).
    let result = eval(
        r#"
        (let ((p (async
                   (define counter 0)
                   (retry (fn ()
                            (set! counter (+ counter 1))
                            (if (< counter 3) (error "not yet") counter))
                          {:max-attempts 5 :base-delay-ms 0}))))
          (await p))
        "#,
    );
    assert_eq!(result, Value::int(3));
}

#[test]
fn retry_exhausted_in_async_context_reraises_last_error() {
    let err = eval_vm_err(
        r#"
        (let ((p (async
                   (retry (fn () (error "always fails")) {:max-attempts 2 :base-delay-ms 0}))))
          (await p))
        "#,
    );
    assert!(
        err.contains("always fails"),
        "expected the exhausted retry's last error to surface, got: {err}"
    );
}

#[test]
fn retry_blocking_compatibility_native_rejects_runtime_entry_before_calling_thunk() {
    let result = eval(
        r#"
        (let ((thunk-calls 0))
          (try
            (__retry-blocking
              (fn ()
                (set! thunk-calls (+ thunk-calls 1))
                :must-not-return)
              {:max-attempts 2 :base-delay-ms 0})
            (catch error (list (:message error) thunk-calls))))
        "#,
    );

    let result = result
        .as_list()
        .expect("runtime guard error should be catchable")
        .to_vec();
    let error = result[0].as_str().expect("guard error message");
    assert!(
        error.contains("__retry-blocking is a host-only adapter; runtime code must use retry"),
        "unexpected blocking-native error: {error}"
    );
    assert_eq!(
        result[1],
        Value::int(0),
        "guard must run before the thunk callback"
    );
}

// === Regression #104: async/spawn keeps captured locals live across the spawn boundary ===
//
// `async/spawn` runs the task on a dedicated task VM whose stack differs from the
// spawning VM's, so a still-open upvalue cell (which indexes the spawning VM's
// stack) must be detached before the task can read it (C1: cells must not dangle
// on a foreign stack). The detach used to snapshot the cell by VALUE at spawn time
// and sever it from the defining frame's slot, so a `set!` to the captured local
// that ran AFTER the spawn but BEFORE the task first read it was lost — the task
// saw the stale spawn-time value. The fix keeps the detached cell TRACKED to the
// still-live defining frame so later `StoreLocal` writes flow into it.
#[test]
fn spawn_observes_set_of_captured_local_after_spawn() {
    // The `set! x 42` runs before the task body ever reads `x` (the task only
    // advances when `await` drives the scheduler), so the task must observe 42.
    assert_eq!(
        eval(
            r#"(define (demo)
                 (define x nil)
                 (define p (async/spawn (fn () (async/sleep 5) x)))
                 (set! x 42)
                 (await p))
               (demo)"#
        ),
        Value::int(42)
    );
}

// Control: the SAME capture + `set!` shape WITHOUT a spawn shares the cell
// correctly (plain C1 open-upvalue semantics). Kept alongside the spawn case so a
// regression that breaks the non-spawn path is distinguishable from a spawn-only
// regression.
#[test]
fn no_spawn_observes_set_of_captured_local() {
    assert_eq!(
        eval(
            r#"(define (demo)
                 (define x nil)
                 (define f (fn () x))
                 (set! x 42)
                 (f))
               (demo)"#
        ),
        Value::int(42)
    );
}

// Multiple post-spawn mutations before the task reads: the task must observe the
// LAST write, not the spawn-time snapshot nor an intermediate value.
#[test]
fn spawn_observes_latest_of_several_post_spawn_writes() {
    assert_eq!(
        eval(
            r#"(define (demo)
                 (define x 0)
                 (define p (async/spawn (fn () (async/sleep 5) x)))
                 (set! x 1)
                 (set! x 2)
                 (set! x 99)
                 (await p))
               (demo)"#
        ),
        Value::int(99)
    );
}

// Two tasks sharing the same captured local both observe the post-spawn write,
// and the deduplicated shared cell stays consistent across both task VMs.
#[test]
fn two_spawns_share_captured_local_after_set() {
    assert_eq!(
        eval(
            r#"(define (demo)
                 (define x 0)
                 (define p (async/spawn (fn () (async/sleep 5) x)))
                 (define q (async/spawn (fn () (async/sleep 5) x)))
                 (set! x 7)
                 (+ (await p) (await q)))
               (demo)"#
        ),
        Value::int(14)
    );
}

// The value captured after a post-spawn `set!` must be usable as a heap value
// (string), exercising that the tracked cell owns its value across the boundary
// (GC-reachability of the tracked value, not just an immediate int).
#[test]
fn spawn_observes_post_spawn_heap_value() {
    assert_eq!(
        eval(
            r#"(define (demo)
                 (define s "before")
                 (define p (async/spawn (fn () (async/sleep 5) s)))
                 (set! s "after")
                 (await p))
               (demo)"#
        ),
        Value::string("after")
    );
}

// === event/select and io/read-key-timeout yield in async context (#88) ===

// event/select must NOT block the cooperative scheduler thread while it waits.
// A task that `event/select`s on a source that never fires (a bogus :proc handle)
// with an 80ms timeout is spawned FIRST; a sibling task that only sends a marker
// is spawned second. If event/select blocked the OS thread for the whole timeout,
// the sibling could not run until the select returned, so the select's marker
// (:select-done) would reach the log channel FIRST. Because it yields, the sibling
// runs to completion the instant the select parks — so :sibling-ran lands first.
// This ordering is deterministic (the sibling does no I/O and no sleep, so it
// finishes immediately once scheduled), which is exactly what distinguishes a
// cooperative yield from a blocking wait.
#[test]
fn event_select_yields_to_sibling_in_async_context() {
    let out = eval(
        r#"
        (let ((log (channel/new 4)))
          (async/all
            (list
              (async
                (event/select (list {:type :proc :handle 999999}) 80)
                (channel/send log :select-done))
              (async
                (channel/send log :sibling-ran))))
          (list (channel/recv log) (channel/recv log)))
    "#,
    );
    assert_eq!(
        out,
        Value::list(vec![
            Value::keyword("sibling-ran"),
            Value::keyword("select-done"),
        ]),
        "event/select must yield so the sibling runs before the select's timeout resolves"
    );
}

// event/select with no ready source still returns nil on timeout from within an
// async context (the cooperative path must preserve the sync return semantics).
#[test]
fn event_select_times_out_to_nil_in_async_context() {
    assert_eq!(
        eval(r#"(await (async (event/select (list {:type :proc :handle 999999}) 20)))"#),
        Value::nil()
    );
}

#[test]
fn timer_only_event_select_preserves_earliest_source() {
    let out = eval(
        r#"
        (let ((event (event/select (list (time/tick 30) (time/tick 5)) 100)))
          (= (:ms (:source event)) 5))
        "#,
    );
    assert_eq!(out, Value::bool(true));
}

// io/read-key-timeout: without a TTY we cannot deterministically feed a keystroke
// nor force a clean idle timeout (a non-TTY stdin is often at EOF, which the first
// poll resolves to nil immediately). So this does NOT prove the yield-during-wait
// behavior — the event/select test above is the deterministic cooperative-yield
// oracle. What it DOES prove: the async-context branch is wired up, returns
// (nil on timeout/EOF, or a key), and neither panics nor deadlocks the scheduler
// while a co-scheduled sibling task also runs to completion. Unix-only because
// io/read-key-timeout is registered only on Unix.
#[cfg(unix)]
#[test]
fn read_key_timeout_async_branch_completes_with_sibling() {
    let out = eval(
        r#"
        (let ((log (channel/new 4)))
          (async/all
            (list
              (async
                (io/read-key-timeout 20)
                (channel/send log :key-done))
              (async
                (channel/send log :sibling-ran))))
          ;; Drain both markers as a set; ordering is NOT asserted (it depends on
          ;; whether stdin is at EOF, which varies by test environment).
          (let ((a (channel/recv log)) (b (channel/recv log)))
            (list (or (= a :key-done) (= b :key-done))
                  (or (= a :sibling-ran) (= b :sibling-ran)))))
    "#,
    );
    assert_eq!(
        out,
        Value::list(vec![Value::bool(true), Value::bool(true)]),
        "both the read-key-timeout task and its sibling must complete without deadlock"
    );
}

// === Unified cooperative runtime characterization ===

#[test]
fn awaited_child_mutation_is_visible_to_parent() {
    // Parent and child must retain one shared lexical cell across suspension.
    assert_eq!(
        eval(
            r#"
            (define (mutate-from-child)
              (define value 0)
              (define child (async/spawn (fn () (set! value 42))))
              (await child)
              value)
            (mutate-from-child)
            "#,
        ),
        Value::int(42),
    );
}

#[test]
fn race_with_settled_winner_does_not_cancel_supplied_loser() {
    assert_eq!(
        eval(
            r#"
            (define loser (async (async/sleep 10) :loser-finished))
            (define winner (async/resolved :winner))
            (define result (async/race (list winner loser)))
            (list result (async/cancelled? loser) (await loser))
            "#,
        ),
        Value::list(vec![
            Value::keyword("winner"),
            Value::bool(false),
            Value::keyword("loser-finished"),
        ]),
    );
}

#[test]
fn sleep_rejects_duration_negative_before_rounding() {
    // A negative input remains invalid even when nearest-integer rounding would produce zero.
    let err = eval_vm_err("(async/sleep -0.4)");
    assert!(
        err.contains("non-negative"),
        "expected non-negative duration error, got: {err}"
    );
}

#[test]
fn timeout_rejects_duration_negative_before_rounding() {
    let err = eval_vm_err("(async/timeout -0.4 (async/resolved :ready))");
    assert!(
        err.contains("non-negative"),
        "expected non-negative duration error, got: {err}"
    );
}

#[test]
fn sleep_rejects_non_finite_durations_cleanly() {
    // NaN and infinities are never valid deadlines.
    for input in [
        "(async/sleep math/nan)",
        "(async/sleep math/infinity)",
        "(async/sleep (- math/infinity))",
    ] {
        let err = eval_vm_err(input);
        assert!(
            err.contains("finite"),
            "expected finite duration error for {input}, got: {err}"
        );
    }
}

#[test]
fn sleep_rejects_overflowing_finite_duration_cleanly() {
    // A finite float outside the duration representation must return a language error.
    let err = eval_vm_err("(async/sleep 1e100)");
    assert!(
        err.contains("exceeds maximum") || err.contains("range"),
        "expected duration range error, got: {err}"
    );
}

#[test]
fn channel_rejects_unrepresentable_capacity_without_panicking() {
    // Capacity validation must reject this before VecDeque attempts an allocation.
    let err = eval_vm_err("(channel/new 9223372036854775807)");
    assert!(
        err.contains("capacity"),
        "expected channel capacity error, got: {err}"
    );
}

#[test]
fn scheduler_workload_beyond_tick_ceiling_completes() {
    // A scheduler safety policy must not reject a finite cooperative workload.
    assert_eq!(
        eval(
            r#"
            (await
              (async
                (let loop ((remaining 1000001))
                  (if (= remaining 0)
                      :complete
                      (begin
                        (async/sleep 0)
                        (loop (- remaining 1)))))))
            "#,
        ),
        Value::keyword("complete"),
    );
}

#[test]
fn nested_aggregate_callback_can_spawn_await_and_resume_parent() {
    // A callback's nested suspension must resume through its parent task without re-entry.
    assert_eq!(
        eval(
            r#"
            (await
              (async
                (map
                  (fn (n)
                    (let ((values
                            (async/all
                              (list (async (+ n 1))
                                    (async (+ n 2))))))
                      (await (async (+ (car values) (cadr values))))))
                  (list 1 10 100))))
            "#,
        ),
        Value::list(vec![Value::int(5), Value::int(23), Value::int(203)]),
    );
}

// === Cooperative callback re-entry: apply / call-with-values / multi-list map ===
//
// `apply`, `call-with-values`, and multi-list `map` drive their callback through
// the structural `NativeOutcome::Call` ABI, so a runtime-only native (async/spawn,
// channel/*, async/resolved) passed as the callback SUSPENDS cleanly instead of
// hitting the value-ABI "requires runtime invocation" stub. Regression for the
// callback-re-entry bug.

#[test]
fn apply_runtime_native_callback_suspends() {
    // `(apply async/spawn (list thunk))` yields an awaitable promise, not an
    // "internal error: runtime native function 'async/spawn' requires runtime
    // invocation".
    assert_eq!(
        eval(r#"(async/await (apply async/spawn (list (fn () 42))))"#),
        Value::int(42)
    );
}

#[test]
fn apply_preserves_synchronous_semantics() {
    // Leading fixed args + spread final list, applied synchronously.
    assert_eq!(eval(r#"(apply + 1 2 (list 3 4))"#), Value::int(10));
}

#[test]
fn apply_structurally_invokes_keyword_callable() {
    assert_eq!(
        eval(r#"(apply :name (list {:name "Ada"}))"#),
        Value::string("Ada")
    );
}

#[test]
fn apply_of_suspending_lambda_runs_cooperatively() {
    // `(apply <lambda> …)` where the lambda body performs a runtime-only async
    // op must suspend + drain like single-list `map`/`foldl`/`call-with-values`,
    // not leak the value-ABI "requires runtime invocation" stub. This is also
    // the "wrap it in a lambda" workaround the graceful nested-apply error
    // suggests, so it must actually work.
    assert_eq!(
        eval(r#"(apply (fn (x) (async/await (async/spawn (fn () (* x 2))))) (list 21))"#),
        Value::int(42)
    );
}

#[test]
fn applied_callback_survives_channel_then_timer_suspension_and_preserves_mutation() {
    assert_eq!(
        eval(
            r#"(let ((seen 0)
                     (stage (channel/new 1)))
                 (channel/send stage :primed)
                 (let ((pending
                         (async
                           (apply
                             (fn ()
                               (set! seen (+ seen 1))
                               (channel/send stage :callback-started)
                               (set! seen (+ seen 10))
                               (async/sleep 5)
                               (set! seen (+ seen 100))
                               seen)
                             (list)))))
                   (let ((markers
                           (await
                             (async
                               (async/sleep 5)
                               (list (channel/recv stage)
                                     (channel/recv stage))))))
                     (list markers (await pending) seen))))"#
        ),
        Value::list(vec![
            Value::list(vec![
                Value::keyword("primed"),
                Value::keyword("callback-started"),
            ]),
            Value::int(111),
            Value::int(111),
        ])
    );
}

#[test]
fn multimethod_selected_method_suspends_cooperatively() {
    // A direct multimethod call `(mm x)` whose SELECTED method suspends
    // (`async/await`/`async/spawn`) must run cooperatively under the runtime
    // instead of leaking the value-ABI "requires runtime invocation" stub —
    // the multimethod half of Step G (see docs/deferred.md). Dispatch itself
    // (the synchronous `(:kind s)` selector) is unaffected; a sibling
    // synchronous method (`:square`) on the SAME multimethod is unaffected too.
    assert_eq!(
        eval(
            r#"(begin
                 (defmulti shape-area (fn (s) (:kind s)))
                 (defmethod shape-area :circle
                   (fn (s) (async/await (async/spawn (fn () (* 3 (:r s) (:r s)))))))
                 (defmethod shape-area :square
                   (fn (s) (* (:side s) (:side s))))
                 (list (shape-area {:kind :circle :r 2})
                       (shape-area {:kind :square :side 4})))"#
        ),
        Value::list(vec![Value::int(12), Value::int(16)])
    );
}

#[test]
fn multimethod_dispatch_function_suspends_cooperatively() {
    assert_eq!(
        eval(
            r#"(begin
                 (defmulti after-sleep async/sleep)
                 (defmethod after-sleep nil (fn (ms) :done))
                 (after-sleep 5))"#
        ),
        Value::keyword("done")
    );
}

#[test]
fn multimethod_dispatch_and_method_preserve_captured_mutation() {
    assert_eq!(
        eval(
            r#"(let ((seen 0))
                 (defmulti mutate-after-wait
                   (fn (key)
                     (set! seen (+ seen 1))
                     (async/sleep 5)
                     key))
                 (defmethod mutate-after-wait :go
                   (fn (key)
                     (set! seen (+ seen 10))
                     (async/sleep 5)
                     seen))
                 (list (mutate-after-wait :go) seen))"#
        ),
        Value::list(vec![Value::int(11), Value::int(11)])
    );
}

#[test]
fn async_multimethod_preserves_defining_frame_capture() {
    assert_eq!(
        eval(
            r#"(let ((seen 0))
                 (defmulti async-captured-multimethod
                   (fn (key)
                     (set! seen (+ seen 1))
                     (async/sleep 5)
                     key))
                 (defmethod async-captured-multimethod :go
                   (fn (key)
                     (set! seen (+ seen 10))
                     (async/sleep 5)
                     seen))
                 (let ((pending (async (async-captured-multimethod :go))))
                   (list (await pending) seen)))"#
        ),
        Value::list(vec![Value::int(11), Value::int(11)])
    );
}

#[test]
fn method_added_after_spawn_snapshots_its_defining_frame_capture() {
    assert_eq!(
        eval(
            r#"(let ((x 41))
                 (defmulti late-method-multimethod (fn (key) key))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           (late-method-multimethod :go))))
                   (defmethod late-method-multimethod :go
                     (fn (key) (+ x 1)))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn default_method_added_after_spawn_snapshots_its_defining_frame_capture() {
    assert_eq!(
        eval(
            r#"(let ((x 41))
                 (defmulti late-default-multimethod (fn (key) key))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           (late-default-multimethod :missing))))
                   (defmethod late-default-multimethod :default
                     (fn (key) (+ x 1)))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn snapshotting_multimethod_self_cycle_terminates() {
    assert_eq!(
        eval(
            r#"(let ((seen 0))
                 (defmulti cyclic-captured-multimethod
                   (fn (key) key))
                 (defmethod cyclic-captured-multimethod :self
                   cyclic-captured-multimethod)
                 (defmethod cyclic-captured-multimethod :go
                   (fn (key)
                     (set! seen (+ seen 1))
                     (async/sleep 5)
                     seen))
                 (let ((pending (async (cyclic-captured-multimethod :go))))
                   (list (await pending) seen)))"#
        ),
        Value::list(vec![Value::int(1), Value::int(1)])
    );
}

#[test]
fn mutable_array_push_after_spawn_snapshots_inserted_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callbacks (mutable-array/new)))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           ((mutable-array/get callbacks 0)))))
                   (mutable-array/push! callbacks (fn () (+ x 1)))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn intrinsic_mutable_array_set_after_spawn_snapshots_inserted_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callbacks (mutable-array/new 1 nil)))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           ((mutable-array/get callbacks 0)))))
                   (mutable-array/set! callbacks 0 (fn () (+ x 1)))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn aliased_mutable_array_set_after_spawn_snapshots_inserted_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callbacks (mutable-array/new 1 nil))
                     (set-callback! mutable-array/set!))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           ((mutable-array/get callbacks 0)))))
                   (set-callback! callbacks 0 (fn () (+ x 1)))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn mutable_cell_set_after_spawn_snapshots_inserted_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callback (mutable-cell/new nil)))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           ((mutable-cell/get callback)))))
                   (mutable-cell/set! callback (fn () (+ x 1)))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn buffered_channel_send_after_spawn_snapshots_sent_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callbacks (channel/new 1)))
                 (let ((pending
                         (async
                           ((channel/recv callbacks)))))
                   (channel/send callbacks (fn () (+ x 1)))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn applied_channel_send_after_spawn_snapshots_sent_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callbacks (channel/new 1)))
                 (let ((pending
                         (async
                           ((channel/recv callbacks)))))
                   (apply channel/send
                     (list callbacks (fn () (+ x 1))))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn synchronous_apply_hof_snapshots_target_native_escaping_args() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callbacks (mutable-array/new)))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           ((mutable-array/get callbacks 0)))))
                   (foldr apply
                     (list callbacks (fn () (+ x 1)))
                     (list mutable-array/push!))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn context_set_before_spawn_inherits_snapshotted_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41))
                 (context/set :cb (fn () (+ x 1)))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           ((context/get :cb)))))
                   (await pending)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn context_set_after_spawn_remains_parent_local() {
    assert_eq!(
        eval(
            r#"(let ((x 41))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           (context/get :cb))))
                   (context/set :cb (fn () (+ x 1)))
                   (list (await pending) ((context/get :cb)))))"#
        ),
        Value::list(vec![Value::nil(), Value::int(42)])
    );
}

#[test]
fn blocked_channel_send_after_spawn_snapshots_handed_off_closure() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (callbacks (channel/new 1)))
                 (channel/send callbacks (fn () :primed))
                 (let ((receiver
                         (async
                           (async/sleep 10)
                           ((begin
                              (channel/recv callbacks)
                              (channel/recv callbacks))))))
                   (channel/send callbacks (fn () (+ x 1)))
                   (await receiver)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn multimethod_structural_stages_propagate_failures() {
    let dispatch_error = eval_vm_err(
        r#"(begin
             (defmulti failing-dispatch
               (fn (key) (async/sleep 5) (error "dispatch boom")))
             (defmethod failing-dispatch :go (fn (key) :unreachable))
             (failing-dispatch :go))"#,
    );
    assert!(
        dispatch_error.contains("dispatch boom"),
        "expected dispatch failure, got: {dispatch_error}"
    );

    let handler_error = eval_vm_err(
        r#"(begin
             (defmulti failing-handler (fn (key) key))
             (defmethod failing-handler :go
               (fn (key) (async/sleep 5) (error "handler boom")))
             (failing-handler :go))"#,
    );
    assert!(
        handler_error.contains("handler boom"),
        "expected handler failure, got: {handler_error}"
    );
}

#[test]
fn cancelling_multimethod_dispatch_cancels_the_parked_call() {
    assert_eq!(
        eval(
            r#"(let ((started (channel/new 1)))
                 (defmulti cancellable-dispatch
                   (fn (key)
                     (channel/send started :started)
                     (async/sleep 1000)
                     key))
                 (defmethod cancellable-dispatch :go (fn (key) :unreachable))
                 (let ((p (async (cancellable-dispatch :go))))
                   (await (async (channel/recv started)))
                   (let ((requested (async/cancel p)))
                     (try (await p) (catch error nil))
                     (list requested (async/cancelled? p)))))"#
        ),
        Value::list(vec![Value::bool(true), Value::bool(true)])
    );
}

#[test]
fn cancelling_async_multimethod_preserves_captured_mutation() {
    assert_eq!(
        eval(
            r#"(let ((seen 0)
                     (started (channel/new 1)))
                 (defmulti cancellable-captured-multimethod
                   (fn (key)
                     (set! seen 1)
                     (channel/send started :started)
                     (async/sleep 1000)
                     key))
                 (defmethod cancellable-captured-multimethod :go
                   (fn (key) :unreachable))
                 (let ((pending
                         (async (cancellable-captured-multimethod :go))))
                   (await (async (channel/recv started)))
                   (async/cancel pending)
                   (try (await pending) (catch error nil))
                   (list seen (async/cancelled? pending))))"#
        ),
        Value::list(vec![Value::int(1), Value::bool(true)])
    );
}

#[test]
fn cancelling_late_method_preserves_captured_mutation() {
    assert_eq!(
        eval(
            r#"(let ((x 41)
                     (started (channel/new 1)))
                 (defmulti late-cancellable-multimethod (fn (key) key))
                 (let ((pending
                         (async
                           (async/sleep 10)
                           (late-cancellable-multimethod :go))))
                   (defmethod late-cancellable-multimethod :go
                     (fn (key)
                       (set! x (+ x 1))
                       (channel/send started :started)
                       (async/sleep 1000)
                       :unreachable))
                   (await (async (channel/recv started)))
                   (async/cancel pending)
                   (try (await pending) (catch error nil))
                   (list x (async/cancelled? pending))))"#
        ),
        Value::list(vec![Value::int(42), Value::bool(true)])
    );
}

#[test]
fn escaping_closure_matches_its_exact_owner_upvalue_cell() {
    let inner = sema_eval::Interpreter::new();
    inner.global_env.set(
        intern("__snapshot-owner-collision"),
        Value::native_fn(
            NativeFn::simple("__snapshot-owner-collision", |args| {
                if args.len() != 1 {
                    return Err(SemaError::arity(
                        "__snapshot-owner-collision",
                        "1",
                        args.len(),
                    ));
                }
                Ok(Value::nil())
            })
            .with_escaping_args(&[0]),
        ),
    );
    OWNER_COLLISION_INNER.with(|slot| {
        *slot.borrow_mut() = Some(inner);
    });

    let outer = sema_eval::Interpreter::new();
    outer.global_env.set(
        intern("__run-owner-collision"),
        Value::native_fn(
            NativeFn::simple("__run-owner-collision", |args| {
                if args.len() != 1 {
                    return Err(SemaError::arity("__run-owner-collision", "1", args.len()));
                }
                OWNER_COLLISION_INNER.with(|slot| {
                    let inner = slot.borrow();
                    let inner = inner
                        .as_ref()
                        .ok_or_else(|| SemaError::eval("owner-collision inner VM is missing"))?;
                    inner
                        .global_env
                        .set(intern("__collision-target"), args[0].clone());
                    inner.eval_str_compiled(
                        r#"(let ((x 900))
                             (let ((keep-open (fn () x)))
                               (__snapshot-owner-collision __collision-target)
                               (list (__collision-target) (keep-open))))"#,
                    )
                })
            })
            .with_escaping_args(&[0]),
        ),
    );

    let result = outer.eval_str_compiled(
        r#"(let ((x 41))
             (__run-owner-collision (fn () (+ x 1))))"#,
    );
    OWNER_COLLISION_INNER.with(|slot| {
        slot.borrow_mut().take();
    });

    assert_eq!(
        result.unwrap(),
        Value::list(vec![Value::int(42), Value::int(900)])
    );
}

#[test]
fn multimethod_continuations_are_isolated_between_interpreters() {
    let left = sema_eval::Interpreter::new();
    let right = sema_eval::Interpreter::new();
    let left_root = left
        .submit_str(
            r#"(begin
                 (defmulti colliding-multimethod async/sleep)
                 (defmethod colliding-multimethod nil (fn (ms) :left))
                 (colliding-multimethod 50))"#,
            RootOptions::default(),
        )
        .expect("left root admitted");
    let right_root = right
        .submit_str(
            r#"(begin
                 (defmulti colliding-multimethod async/sleep)
                 (defmethod colliding-multimethod nil (fn (ms) :right))
                 (colliding-multimethod 50))"#,
            RootOptions::default(),
        )
        .expect("right root admitted");

    for _ in 0..8 {
        left.drive_turn().expect("left runtime progresses");
        right.drive_turn().expect("right runtime progresses");
    }
    assert!(matches!(left_root.poll_result(), RootPoll::Pending));
    assert!(matches!(right_root.poll_result(), RootPoll::Pending));

    assert!(left_root.cancel(CancelReason::Explicit));
    while matches!(left_root.poll_result(), RootPoll::Pending) {
        left.drive_turn().expect("left cancellation settles");
    }
    assert!(matches!(
        left_root.poll_result(),
        RootPoll::Ready(settlement)
            if matches!(settlement.outcome, TaskOutcome::Cancelled(CancelReason::Explicit))
    ));
    assert!(matches!(right_root.poll_result(), RootPoll::Pending));
    assert_eq!(
        right
            .drive_until_settled(&right_root)
            .expect("right runtime remains isolated"),
        Value::keyword("right")
    );
}

#[test]
fn apply_of_suspending_multimethod_runs_cooperatively() {
    assert_eq!(
        eval(
            r#"(begin
                 (defmulti shape-area (fn (s) (:kind s)))
                 (defmethod shape-area :circle
                   (fn (s) (async/await (async/spawn (fn () (* 3 (:r s) (:r s)))))))
                 (apply shape-area (list {:kind :circle :r 2})))"#
        ),
        Value::int(12)
    );
}

#[test]
fn nested_apply_of_runtime_native_runs_structurally() {
    assert_eq!(
        eval(r#"(async/await (apply apply (list async/spawn (list (fn () 1)))))"#),
        Value::int(1)
    );
    assert_eq!(
        eval(
            r#"(async/await
                 (apply call-with-values (list (fn () 5) async/resolved)))"#
        ),
        Value::int(5)
    );
}

#[test]
fn apply_channel_send_callback_runs() {
    // A runtime-only op applied over a channel runs cooperatively (no stub error).
    assert_eq!(
        eval(
            r#"(let ((c (channel/new 1)))
                 (apply channel/send (list c 7))
                 (channel/recv c))"#
        ),
        Value::int(7)
    );
}

// `async/sleep` is dual-ABI: cooperative callback drivers structurally invoke
// its Timer suspend, while host-only synchronous entry points use its plain
// value ABI. No TLS yield signal bridges the two paths.

#[test]
fn sleep_passed_directly_to_cooperative_predicate_hof_suspends() {
    assert_eq!(eval(r#"(any async/sleep (list 5))"#), Value::bool(false));
}

#[test]
fn apply_drives_dual_abi_native_structurally() {
    let out = eval(
        r#"
        (let ((events (mutable-array/new)))
          (async/all
            (list
              (async
                (apply async/sleep (list 20))
                (mutable-array/push! events :slept))
              (async
                (mutable-array/push! events :sibling))))
          (mutable-array/->vector events))
        "#,
    );
    assert_eq!(
        out,
        Value::vector(vec![Value::keyword("sibling"), Value::keyword("slept")]),
        "the dual-ABI sleep must park the applied call so its sibling runs"
    );
}

#[test]
fn cancelling_structural_apply_cancels_the_parked_call() {
    assert_eq!(
        eval(
            r#"(let ((started (channel/new 1)))
                 (let ((p (async
                            (channel/send started :started)
                            (apply async/sleep (list 1000)))))
                   (await (async (channel/recv started)))
                   (let ((requested (async/cancel p)))
                     (try (await p) (catch error nil))
                     (list requested (async/cancelled? p)))))"#
        ),
        Value::list(vec![Value::bool(true), Value::bool(true)])
    );
}

#[test]
fn structural_apply_propagates_native_failure() {
    let err = eval_vm_err(r#"(apply error (list "apply boom"))"#);
    assert!(
        err.contains("apply boom"),
        "expected the applied native failure, got: {err}"
    );
}

#[test]
fn sleep_wrapped_in_predicate_callback_suspends() {
    assert_eq!(
        eval(r#"(any (fn (x) (async/sleep x) #t) (list 10))"#),
        Value::bool(true)
    );
}

#[test]
fn sleep_passed_directly_to_cooperative_hof_still_suspends() {
    // `map`/`filter`/`sort-by` (`register_hof`, dual-ABI) drive their callback
    // cooperatively under an active runtime quantum, so a raw `async/sleep`
    // suspends structurally instead of reaching the legacy value ABI at all.
    assert_eq!(
        eval(r#"(map async/sleep (list 5 5))"#),
        Value::list(vec![Value::nil(), Value::nil()])
    );
}

#[test]
fn call_with_values_consumer_runtime_native_suspends() {
    // The consumer is a runtime-only op; it suspends cleanly.
    assert_eq!(
        eval(r#"(async/await (call-with-values (fn () 7) async/resolved))"#),
        Value::int(7)
    );
}

#[test]
fn call_with_values_multi_value_spread_preserved() {
    // A multi-value producer spreads its values as the consumer's args.
    assert_eq!(
        eval(r#"(call-with-values (fn () (values 1 2 3)) +)"#),
        Value::int(6)
    );
}

#[test]
fn call_with_values_producer_runtime_native_suspends() {
    // The PRODUCER runs a runtime-only op (await inside it) and suspends cleanly;
    // its result flows to the consumer.
    assert_eq!(
        eval(
            r#"(call-with-values
                 (fn () (async/await (async/spawn (fn () 40))))
                 (fn (x) (+ x 2)))"#
        ),
        Value::int(42)
    );
}

#[test]
fn map_multi_list_runtime_native_callback_runs() {
    // Multi-list `map` with a runtime-only callback runs cooperatively; the
    // channel receives the sent value.
    assert_eq!(
        eval(
            r#"(let ((c (channel/new 1)))
                 (map channel/send (list c) (list 5))
                 (channel/recv c))"#
        ),
        Value::int(5)
    );
}

#[test]
fn map_multi_list_preserves_zip_semantics() {
    // Shortest-list truncation + input order, zipped column-wise.
    assert_eq!(
        eval(r#"(map (fn (a b) (+ a b)) (list 1 2 3) (list 10 20))"#),
        Value::list(vec![Value::int(11), Value::int(22)])
    );
}

#[test]
fn map_multi_list_callback_can_await() {
    // A runtime op inside a multi-list map callback suspends/resumes per column.
    assert_eq!(
        eval(
            r#"(await
                 (async
                   (map (fn (a b) (await (async (+ a b))))
                        (list 1 2 3) (list 100 200 300))))"#
        ),
        Value::list(vec![Value::int(101), Value::int(202), Value::int(303)])
    );
}

// === async/run — self-resolving-waits barrier (ASYNC-RUN-BARRIER-1) ===
//
// These use the out-of-process watchdog (a real wall-clock kill) so a barrier
// regression that reintroduces a hang shows as `timed_out`, not a silent pass.

/// `(async/run)` waits for a descendant parked on a REAL timer to fire (a
/// self-resolving wait) before releasing: "bg" must print before "after-run".
/// The pre-fix ready-drain printed "after-run" first (or dropped "bg" entirely).
#[test]
fn async_run_waits_for_timer_parked_descendant() {
    let run = common::watchdog::run_sema_with_timeout(
        r#"(begin
             (async/spawn (fn () (async/sleep 30) (println "bg")))
             (async/run)
             (println "after-run"))"#,
        std::time::Duration::from_secs(15),
    );
    assert!(!run.timed_out, "async/run hung; stderr:\n{}", run.stderr);
    assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
    let bg = run.stdout.find("bg");
    let after = run.stdout.find("after-run");
    assert!(
        bg.is_some() && after.is_some() && bg < after,
        "expected `bg` before `after-run`, got stdout:\n{}",
        run.stdout
    );
}

/// Transitivity: a spawned child that itself spawns a sleeping grandchild. The
/// barrier keeps waiting until the grandchild's self-resolving timer fires, so
/// "grandchild" prints before "after-run".
#[test]
fn async_run_drains_transitively_spawned_sleeper() {
    let run = common::watchdog::run_sema_with_timeout(
        r#"(begin
             (async/spawn
               (fn ()
                 (async/spawn (fn () (async/sleep 30) (println "grandchild")))
                 (println "child")))
             (async/run)
             (println "after-run"))"#,
        std::time::Duration::from_secs(15),
    );
    assert!(!run.timed_out, "async/run hung; stderr:\n{}", run.stderr);
    assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
    let grandchild = run.stdout.find("grandchild");
    let after = run.stdout.find("after-run");
    assert!(
        grandchild.is_some() && after.is_some() && grandchild < after,
        "expected `grandchild` before `after-run`, got stdout:\n{}",
        run.stdout
    );
}

/// The AWAITER of a self-resolving sleeper must also complete before
/// `(async/run)` returns — not just the sleeper itself. A settled sleeper
/// removes its task and queues its promise wake in `pending`; the barrier check
/// must not run at that intermediate point (the awaiter still looks parked on a
/// cycle-forming Promise) and release early. Regression for the deferred-wake
/// window: `B-got` (after `(async/await p)` on a spawned sleeper) must print
/// before `after-run`.
#[test]
fn async_run_waits_for_awaiter_of_transitive_sleeper() {
    let run = common::watchdog::run_sema_with_timeout(
        r#"(begin
             (async/spawn
               (fn ()
                 (let ((p (async/spawn (fn () (async/sleep 40) 99))))
                   (async/await p)
                   (println "B-got"))))
             (async/run)
             (println "after-run"))"#,
        std::time::Duration::from_secs(15),
    );
    assert!(!run.timed_out, "async/run hung; stderr:\n{}", run.stderr);
    assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
    let b_got = run.stdout.find("B-got");
    let after = run.stdout.find("after-run");
    assert!(
        b_got.is_some() && after.is_some() && b_got < after,
        "expected `B-got` (awaiter of a sleeper) before `after-run`, got stdout:\n{}",
        run.stdout
    );
}

/// A NESTED `(async/run)` (inside a spawned task) shares its spawner's origin
/// root with the outer `(async/run)`. The outer barrier must WAIT for the inner
/// one (a descendant sub-graph) rather than race it: if both released at once
/// the outer could win, settle the root, and drop the inner task's
/// continuation. Deterministic result `6` (inner task increments after its own
/// nested drain); a regression showed a nondeterministic `5`.
#[test]
fn nested_async_run_waits_for_inner_barrier() {
    for _ in 0..8 {
        let run = common::watchdog::run_sema_with_timeout(
            r#"(let ((c (mutable-cell/new 0)))
                 (async/spawn
                   (fn ()
                     (async/spawn (fn () (async/sleep 20) (mutable-cell/set! c 5)))
                     (async/run)
                     (mutable-cell/set! c (+ (mutable-cell/get c) 1))))
                 (async/run)
                 (println (mutable-cell/get c)))"#,
            std::time::Duration::from_secs(15),
        );
        assert!(
            !run.timed_out,
            "nested async/run hung; stderr:\n{}",
            run.stderr
        );
        assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
        assert!(
            run.stdout.trim().ends_with('6'),
            "expected inner continuation to run (6), got stdout:\n{}",
            run.stdout
        );
    }
}

/// Barrier ordering is by spawn order (`TaskId`), not park order. When the OUTER
/// task suspends (here `async/sleep`) BEFORE reaching its own `(async/run)`, the
/// inner descendant barrier parks FIRST — so a park-order key would invert the
/// nesting and let the outer release first, dropping the inner continuation.
/// TaskId order (a descendant's id always exceeds its ancestor's) keeps the
/// outer waiting for the inner. Deterministic `6`.
#[test]
fn nested_async_run_orders_by_spawn_not_park() {
    for _ in 0..8 {
        let run = common::watchdog::run_sema_with_timeout(
            r#"(let ((c (mutable-cell/new 0)))
                 (async/spawn
                   (fn ()
                     (async/spawn (fn () (async/sleep 50) (mutable-cell/set! c 5)))
                     (async/run)
                     (mutable-cell/set! c (+ (mutable-cell/get c) 1))))
                 (async/sleep 10)
                 (async/run)
                 (println (mutable-cell/get c)))"#,
            std::time::Duration::from_secs(15),
        );
        assert!(!run.timed_out, "async/run hung; stderr:\n{}", run.stderr);
        assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
        assert!(
            run.stdout.trim().ends_with('6'),
            "expected inner continuation to run (6) despite the outer suspending first, got:\n{}",
            run.stdout
        );
    }
}

/// Nested-barrier ordering must survive a REAPED intermediate spawner. Task A
/// spawns B (which runs its own `(async/run)`) and returns — A settles and is
/// removed before B's barrier resolves. The outer `(async/run)` must still wait
/// for B. A spawn-graph walk would break here (B's parent A is gone); the
/// park-order seq stamp does not. Deterministic `6`.
#[test]
fn nested_async_run_waits_across_reaped_parent() {
    for _ in 0..8 {
        let run = common::watchdog::run_sema_with_timeout(
            r#"(let ((c (mutable-cell/new 0)))
                 (async/spawn
                   (fn ()
                     (async/spawn
                       (fn ()
                         (async/spawn (fn () (async/sleep 25) (mutable-cell/set! c 5)))
                         (async/run)
                         (mutable-cell/set! c (+ (mutable-cell/get c) 1))))))
                 (async/run)
                 (println (mutable-cell/get c)))"#,
            std::time::Duration::from_secs(15),
        );
        assert!(
            !run.timed_out,
            "reaped-parent nested async/run hung; stderr:\n{}",
            run.stderr
        );
        assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
        assert!(
            run.stdout.trim().ends_with('6'),
            "expected inner continuation to run across reaped parent (6), got stdout:\n{}",
            run.stdout
        );
    }
}

/// A detached child blocked SENDING on a full channel that only the barrier
/// caller would drain is a channel-rendezvous cycle: the barrier must NOT wait
/// on it (Channel is cycle-forming), so `(async/run)` RELEASES and "released"
/// prints. If Channel were treated self-resolving this would hang → timeout.
#[test]
fn async_run_releases_over_channel_rendezvous_blocked_child() {
    let run = common::watchdog::run_sema_with_timeout(
        r#"(begin
             (let ((ch (channel/new 1)))
               (channel/send ch :fill)
               (async/spawn (fn () (channel/send ch :blocked)))
               (async/run)
               (println "released")))"#,
        std::time::Duration::from_secs(15),
    );
    assert!(
        !run.timed_out,
        "async/run hung on a rendezvous-blocked child; stderr:\n{}",
        run.stderr
    );
    assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
    assert!(
        run.stdout.contains("released"),
        "expected `released`, got stdout:\n{}",
        run.stdout
    );
}

/// A parent that `await`s the very task calling `(async/run)` is a self-await
/// cycle (Promise is cycle-forming): the inner `(async/run)` must release rather
/// than wait on the parent that waits on it. Result flows out as 7.
#[test]
fn async_run_releases_under_self_awaiting_parent() {
    let run = common::watchdog::run_sema_with_timeout(
        r#"(println (async/await (async/spawn (fn () (async/run) 7))))"#,
        std::time::Duration::from_secs(15),
    );
    assert!(
        !run.timed_out,
        "async/run hung on self-await; stderr:\n{}",
        run.stderr
    );
    assert!(run.status.success(), "run failed; stderr:\n{}", run.stderr);
    assert!(
        run.stdout.contains('7'),
        "expected `7`, got stdout:\n{}",
        run.stdout
    );
}

// === Task 0c-7: in-place channel-rendezvous handoff ===

/// A `channel/send`/`recv` pair that resolves immediately loops in place on
/// the SAME `vm` object instead of parking through the pending queue (Task
/// 0c-7). Guard against that in-place loop becoming unbounded and starving a
/// sibling root: drive a tight two-task ping-pong under a TINY per-quantum
/// instruction budget alongside an unrelated sibling root, and confirm the
/// sibling settles within a handful of `drive()` turns while the ping-pong is
/// still mid-flight — proving the handoff loop is bounded by
/// `remaining_budget` (mirroring Task C's `invoke_vm_callback_loop` budget
/// continuation) exactly like a single ordinary quantum always was, not that
/// the sibling merely happened to finish first.
///
/// Needs direct `Runtime`/`DriveBudget` control the `eval()` helper doesn't
/// expose, so it drives `sema_eval::Interpreter`'s runtime by hand rather
/// than going through a `.sema` source string + the CLI.
#[test]
fn channel_pingpong_handoff_respects_instruction_budget_and_lets_sibling_progress() {
    use sema_vm::runtime::{DriveBudget, RootPoll};

    let interp = sema_eval::Interpreter::new();
    let runtime = interp.runtime();

    let ping = sema_reader::read_many(
        r#"
        (let ((c1 (channel/new 1)) (c2 (channel/new 1)))
          (let ((p1 (async (let loop ((n 0))
                              (if (< n 20000)
                                  (begin (channel/send c1 n) (channel/recv c2) (loop (+ n 1)))
                                  n))))
                (p2 (async (let loop ((n 0))
                              (if (< n 20000)
                                  (begin (channel/recv c1) (channel/send c2 n) (loop (+ n 1)))
                                  n)))))
            (+ (await p1) (await p2))))
        "#,
    )
    .expect("pingpong source parses");
    let prog = sema_vm::compile_program(&ping, None).expect("pingpong compiles");
    let mut vm = sema_vm::VM::new(
        interp.global_env.clone(),
        prog.functions,
        &prog.native_table,
        prog.main_cache_slots,
    )
    .expect("pingpong VM builds against stdlib globals");
    vm.seed_main_frame(prog.closure);
    let pingpong = runtime.submit_root(vm).expect("pingpong root admitted");

    let sibling_form = sema_reader::read_many("(+ 1 2)").expect("sibling source parses");
    let sibling_prog = sema_vm::compile_program(&sibling_form, None).expect("sibling compiles");
    let mut sibling_vm = sema_vm::VM::new(
        interp.global_env.clone(),
        sibling_prog.functions,
        &sibling_prog.native_table,
        sibling_prog.main_cache_slots,
    )
    .expect("sibling VM builds");
    sibling_vm.seed_main_frame(sibling_prog.closure);
    let sibling = runtime
        .submit_root(sibling_vm)
        .expect("sibling root admitted");

    let budget = DriveBudget {
        work_item_limit: std::num::NonZeroUsize::new(4096).unwrap(),
        completion_limit: std::num::NonZeroUsize::new(64).unwrap(),
        timer_limit: std::num::NonZeroUsize::new(64).unwrap(),
        root_visit_limit: std::num::NonZeroUsize::new(64).unwrap(),
        cleanup_limit: std::num::NonZeroUsize::new(64).unwrap(),
        // Tiny on purpose: a handful of channel round trips' worth, so a
        // handoff loop that ignored the budget would run away for many, many
        // drive() turns before this test's bound catches it.
        instruction_limit_per_task: std::num::NonZeroUsize::new(200).unwrap(),
        wall_clock_limit: std::time::Duration::from_secs(10),
    };

    let mut turns = 0;
    while matches!(sibling.poll_result(), RootPoll::Pending) {
        runtime.drive(&budget).expect("drive turn succeeds");
        turns += 1;
        assert!(
            turns < 200,
            "sibling root starved by the ping-pong handoff loop after {turns} drive() turns"
        );
    }
    assert!(
        matches!(pingpong.poll_result(), RootPoll::Pending),
        "pingpong should still be mid-flight when the sibling settles — proves the sibling was \
         genuinely interleaved in, not that it merely happened to finish first"
    );
    let RootPoll::Ready(settlement) = sibling.poll_result() else {
        panic!("sibling settles")
    };
    assert!(
        matches!(&settlement.outcome, sema_core::runtime::TaskOutcome::Returned(v) if v.as_int() == Some(3)),
        "sibling settlement: {:?}",
        settlement.outcome
    );
}

/// `channel_pingpong_handoff_respects_instruction_budget_and_lets_sibling_progress`
/// above pins the SAME budget continuation, but a cap-1 ping-pong parks
/// naturally after at most two handoff iterations per round trip (the
/// channel's own capacity forces a genuine suspend), so the in-place loop
/// there never spins more than a couple of iterations regardless of whether
/// `remaining_budget` is honored — it would stay green even if the
/// continuation were defeated entirely. Pin the bound with a scenario that
/// genuinely spins MANY handoff iterations in a single `run_parked_quantum`
/// call: a tight send loop into a channel with capacity far larger than the
/// number of sends, so every `channel/send` resolves immediately via
/// `try_channel_handoff` and the Rust-level loop in `run_parked_quantum`
/// never breaks out to a genuine park — only `remaining_budget` running out
/// stops it.
///
/// The discriminator is turn count, not "who settles first": submitting a
/// trivial sibling root turns out NOT to distinguish the two behaviors on its
/// own, because `channel/new` itself is a genuine multi-step suspend (a
/// `RuntimeRequest`, not a channel wait), so the very first `drive()` turn
/// naturally round-robins the sibling in before the send loop even starts —
/// regardless of whether the later loop is budget-bounded. What the budget
/// continuation actually controls is how many sends land PER TURN once the
/// loop is running: with it intact, `drive()` reports `instructions` pinned
/// at the per-task cap on every turn and needs on the order of
/// `50_000 / (sends per budget)` turns to drain the loop; with it defeated,
/// the whole 50k-send loop completes inline within a single work item (the
/// loop never returns control to `drive()`'s own per-turn accounting, which
/// only checks between work items) and the sender settles within a small
/// handful of turns regardless of the (tiny) per-task instruction budget.
#[test]
fn channel_send_loop_handoff_respects_instruction_budget_and_lets_sibling_progress() {
    use sema_vm::runtime::{DriveBudget, RootPoll};

    let interp = sema_eval::Interpreter::new();
    let runtime = interp.runtime();

    let sender_form = sema_reader::read_many(
        r#"
        (let ((c (channel/new 100000)))
          (let loop ((n 0))
            (if (< n 50000)
                (begin (channel/send c n) (loop (+ n 1)))
                n)))
        "#,
    )
    .expect("sender source parses");
    let sender_prog = sema_vm::compile_program(&sender_form, None).expect("sender compiles");
    let mut sender_vm = sema_vm::VM::new(
        interp.global_env.clone(),
        sender_prog.functions,
        &sender_prog.native_table,
        sender_prog.main_cache_slots,
    )
    .expect("sender VM builds against stdlib globals");
    sender_vm.seed_main_frame(sender_prog.closure);
    let sender = runtime
        .submit_root(sender_vm)
        .expect("sender root admitted");

    let sibling_form = sema_reader::read_many("(+ 1 2)").expect("sibling source parses");
    let sibling_prog = sema_vm::compile_program(&sibling_form, None).expect("sibling compiles");
    let mut sibling_vm = sema_vm::VM::new(
        interp.global_env.clone(),
        sibling_prog.functions,
        &sibling_prog.native_table,
        sibling_prog.main_cache_slots,
    )
    .expect("sibling VM builds");
    sibling_vm.seed_main_frame(sibling_prog.closure);
    let sibling = runtime
        .submit_root(sibling_vm)
        .expect("sibling root admitted");

    let budget = DriveBudget {
        work_item_limit: std::num::NonZeroUsize::new(4096).unwrap(),
        completion_limit: std::num::NonZeroUsize::new(64).unwrap(),
        timer_limit: std::num::NonZeroUsize::new(64).unwrap(),
        root_visit_limit: std::num::NonZeroUsize::new(64).unwrap(),
        cleanup_limit: std::num::NonZeroUsize::new(64).unwrap(),
        // Tiny relative to the 50k-send loop: with the continuation intact this
        // caps each turn to roughly instruction_limit_per_task / cost-per-send
        // sends, so draining the whole loop legitimately takes hundreds of
        // turns. A handoff loop that ignored the budget drains the entire
        // 50k-send loop inline within a couple of turns regardless of this
        // value (see the doc comment above).
        instruction_limit_per_task: std::num::NonZeroUsize::new(500).unwrap(),
        wall_clock_limit: std::time::Duration::from_secs(30),
    };

    let mut turns = 0;
    let mut sibling_settled_at_turn = None;
    while matches!(sender.poll_result(), RootPoll::Pending) {
        runtime.drive(&budget).expect("drive turn succeeds");
        turns += 1;
        if sibling_settled_at_turn.is_none() && matches!(sibling.poll_result(), RootPoll::Ready(_))
        {
            sibling_settled_at_turn = Some(turns);
        }
        assert!(
            turns < 5000,
            "sender never finished draining its 50k-send loop after {turns} drive() turns"
        );
    }

    let sibling_settled_at_turn = sibling_settled_at_turn
        .expect("sibling must have settled by the time the sender's send loop finished");
    assert!(
        sibling_settled_at_turn < turns,
        "sibling only settled at turn {sibling_settled_at_turn}, the SAME turn the sender's \
         send loop finished (turn {turns}) — not a meaningful proof the sibling was \
         interleaved into the loop rather than merely following it"
    );
    assert!(
        turns > 100,
        "the 50k-send loop drained in only {turns} drive() turns — the in-place channel \
         handoff loop is not being chunked by the per-task instruction budget the way it \
         should be. A handoff loop that ignored `remaining_budget` would run the whole \
         loop to completion inline within a single work item and settle in only a \
         couple of turns, regardless of how tiny instruction_limit_per_task is set here"
    );

    let RootPoll::Ready(settlement) = sibling.poll_result() else {
        panic!("sibling settles")
    };
    assert!(
        matches!(&settlement.outcome, sema_core::runtime::TaskOutcome::Returned(v) if v.as_int() == Some(3)),
        "sibling settlement: {:?}",
        settlement.outcome
    );
    let RootPoll::Ready(sender_settlement) = sender.poll_result() else {
        panic!("sender settles")
    };
    assert!(
        matches!(&sender_settlement.outcome, sema_core::runtime::TaskOutcome::Returned(v) if v.as_int() == Some(50000)),
        "sender settlement: {:?}",
        sender_settlement.outcome
    );
}
