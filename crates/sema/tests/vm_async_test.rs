mod common;

use common::eval;
use sema_core::{Caps, Sandbox, Value};

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
    let err = eval_vm_err(r#"(async/await (async/rejected "oops"))"#);
    assert!(
        err.contains("rejected"),
        "expected rejection error, got: {err}"
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

// Records the real-sleep durations the scheduler requests, so a native test can
// prove the blocking-sleep hook fires (the playground Web Worker installs an
// Atomics.wait callback here; native normally uses the std::thread::sleep
// default). fn-pointer callbacks can't capture, so record into a thread-local.
thread_local! {
    static SLEEP_DELTAS: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn record_sleep(ms: u64) {
    SLEEP_DELTAS.with(|d| d.borrow_mut().push(ms));
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
fn blocking_sleep_hook_receives_clock_advances() {
    SLEEP_DELTAS.with(|d| d.borrow_mut().clear());
    sema_core::set_blocking_sleep_callback(record_sleep);

    // Sleeps 10/20/30 across three tasks: the virtual clock advances
    // 0->10->20->30, so the hook should be invoked with deltas summing to 30
    // (total virtual time), and ordering must still be a,b,c.
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

    // Restore the default before asserting so a failure can't leak the hook
    // into another test sharing this thread.
    sema_core::clear_blocking_sleep_callback();
    let deltas = SLEEP_DELTAS.with(|d| d.borrow().clone());

    assert_eq!(
        out,
        Value::list(vec![
            Value::keyword("a"),
            Value::keyword("b"),
            Value::keyword("c"),
        ]),
        "ordering must be unaffected by the blocking-sleep hook"
    );
    assert!(
        !deltas.is_empty(),
        "blocking-sleep hook should have been invoked"
    );
    assert_eq!(
        deltas.iter().sum::<u64>(),
        30,
        "total real-sleep requested should equal total virtual time, got {deltas:?}"
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
    // The tree-walker is retired; every eval entry point runs on the VM, so
    // async/await is accepted via the default `eval_str` path (it used to error
    // with "requires the VM backend" on the tree-walker).
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
    let err = eval_vm_err(r#"(async/timeout 1000 (async/rejected "oops"))"#);
    assert!(err.contains("rejected"), "expected rejection, got: {err}");
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
fn native_callback_passed_directly_raises_clear_error() {
    // A yielding native fn (channel/recv) passed directly as a HOF callback
    // can't propagate its yield through the HOF's Rust loop. Instead of
    // silently dropping yields and producing wrong results, surface a clear
    // error that tells the user to wrap in a lambda.
    let err = eval_vm_err(
        r#"
        (let ((ch (channel/new 1)))
          (let ((producer (async
                            (channel/send ch 1)
                            (channel/send ch 2)
                            (channel/close ch)))
                (consumer (async (map channel/recv (list ch ch ch)))))
            (await consumer)))
        "#,
    );
    assert!(
        err.contains("wrap it in a lambda") || err.contains("wrap in a lambda"),
        "expected lambda-wrap hint, got: {err}"
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

// Regression: nested await on a rejected promise must not double the
// "async/await: task rejected: " prefix in the error message (A2).
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
        !err.contains("task rejected: task rejected"),
        "expected single prefix, got: {err}"
    );
    assert!(
        err.contains("task rejected"),
        "expected rejection message, got: {err}"
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
