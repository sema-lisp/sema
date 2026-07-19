use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    ChannelId, ChannelOperation, ChannelQuery, ChannelReceive, ChannelSend, ChannelWait,
    NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, NativeSuspend, PromiseId,
    PromiseSetMode, PromiseSetWait, ResumeInput, RuntimeRequest, RuntimeResponse, TaskOutcome,
    TaskSettlement, Trace, WaitKind,
};
use sema_core::{check_arity, in_runtime_quantum, Env, NativeFn, SemaError, Value, ValueView};

use crate::register_fn;

/// Parse a duration-in-milliseconds argument for `async/sleep` / `async/timeout`.
/// Accepts an int OR a float (rounded to the nearest whole ms): a duration is
/// naturally a number, and being strict about int-vs-float here is a papercut —
/// `(round …)`, `(math/random)` and ordinary arithmetic routinely yield floats.
fn duration_ms(value: &Value, who: &str) -> Result<i64, SemaError> {
    if let Some(i) = value.as_int() {
        if i < 0 {
            return Err(SemaError::eval(format!(
                "{who}: duration must be non-negative"
            )));
        }
        Ok(i)
    } else if let Some(f) = value.as_float() {
        if !f.is_finite() {
            return Err(SemaError::eval(format!(
                "{who}: duration must be a finite number"
            )));
        }
        // Reject negatives BEFORE rounding: `round(-0.4)` is `-0.0`, so a
        // rounded-then-checked path would silently accept a negative duration.
        if f < 0.0 {
            return Err(SemaError::eval(format!(
                "{who}: duration must be non-negative"
            )));
        }
        Ok(f.round() as i64)
    } else {
        Err(SemaError::type_error("number", value.type_name()))
    }
}

/// Format the `await`-on-cancelled-promise error as a structured, catchable
/// `:cancelled` condition (not a plain rejection): `(:type e)` on the caught
/// value is `:cancelled`. The promise carries no `CancelReason`, so a generic
/// `Explicit` reason is used. Mirrors `runtime::state::await_cancelled_error`.
fn cancelled_error() -> SemaError {
    SemaError::cancelled_condition(
        "async/await: awaited task was cancelled",
        sema_core::runtime::CancelReason::Explicit,
        None,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Validate and cap `async/sleep`'s duration argument, returning the sleep in
/// whole milliseconds. Shared by the plain value and runtime ABIs.
fn sleep_duration_ms(args: &[Value]) -> Result<u64, SemaError> {
    check_arity!(args, "async/sleep", 1);
    let ms = duration_ms(&args[0], "async/sleep")?;
    // Cap the duration (mirrors async/timeout). The runtime's virtual
    // clock jumps straight to a sleeper's wake time and, on native, waits that
    // whole delta in one `thread::sleep`; without a bound an out-of-range
    // duration would wedge the thread for years and could overflow the clock.
    const MAX_SLEEP_MS: i64 = 86_400_000; // 1 day
    if ms > MAX_SLEEP_MS {
        return Err(SemaError::eval(format!(
            "async/sleep: duration {ms} ms exceeds maximum {MAX_SLEEP_MS} ms (1 day)"
        ))
        .with_hint("use a shorter sleep, or loop with smaller sleeps"));
    }
    Ok(ms as u64)
}

/// Continuation for `async/sleep` under the unified runtime. A timer wait carries
/// no value, so a normal fire resumes the parked frame with nil; a cancellation
/// or failure while sleeping propagates the corresponding error. Holds no state.
struct SleepCont;

impl Trace for SleepCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for SleepCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            ResumeInput::Returned(_) | ResumeInput::Runtime(_) => {
                Ok(NativeOutcome::Return(Value::nil()))
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────

/// Extract the runtime `PromiseId` from an `AsyncPromise` handle value. The
/// registry (not the handle) owns the promise's state; every promise op that
/// observes or waits on a promise threads this id to the runtime.
fn expect_promise(args: &[Value], _name: &str, idx: usize) -> Result<PromiseId, SemaError> {
    match args[idx].view() {
        ValueView::AsyncPromise(p) => Ok(p.id),
        _ => Err(SemaError::type_error_with_value(
            "async-promise",
            args[idx].type_name(),
            &args[idx],
        )),
    }
}

/// Extract the runtime `ChannelId` from a `Channel` handle value. The registry
/// (not the handle) owns the buffer/state; every channel op threads this id to
/// the runtime.
fn expect_channel(args: &[Value], _name: &str, idx: usize) -> Result<ChannelId, SemaError> {
    match args[idx].view() {
        ValueView::Channel(c) => Ok(c.id),
        _ => Err(SemaError::type_error_with_value(
            "channel",
            args[idx].type_name(),
            &args[idx],
        )),
    }
}

/// Extract items from a list or vector, or return a type error.
fn expect_list_or_vector<'a>(val: &'a Value, name: &str) -> Result<&'a [Value], SemaError> {
    if let Some(items) = val.as_list() {
        Ok(items)
    } else if let Some(items) = val.as_vector() {
        Ok(items)
    } else {
        Err(SemaError::type_error("list or vector", val.type_name())
            .with_hint(format!("{name} expects a list or vector of promises")))
    }
}

// ── Registration ─────────────────────────────────────────────────

pub fn register(env: &Env) {
    register_predicates(env);
    register_promise_ops(env);
    register_channel_ops(env);
}

// ── Predicates ───────────────────────────────────────────────────

fn register_predicates(env: &Env) {
    register_fn(env, "async/promise?", |args| {
        check_arity!(args, "async/promise?", 1);
        Ok(Value::bool(args[0].is_async_promise()))
    });

    register_fn(env, "channel?", |args| {
        check_arity!(args, "channel?", 1);
        Ok(Value::bool(args[0].is_channel()))
    });
}

// ── Promise operations ───────────────────────────────────────────

/// Register a promise op as a structural runtime native. Plain value-ABI calls
/// fail because promise operations require an active runtime task context.
fn register_runtime_fn(env: &Env, name: &str, f: impl Fn(&[Value]) -> NativeResult + 'static) {
    register_runtime_fn_with_escaping_args(env, name, &[], f);
}

fn register_runtime_fn_with_escaping_args(
    env: &Env,
    name: &str,
    escaping_args: &'static [usize],
    f: impl Fn(&[Value]) -> NativeResult + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::simple_result(name, f).with_escaping_args(escaping_args)),
    );
}

/// Map a settled promise's outcome to a native result: a returned value resumes
/// the caller, a failure re-raises the PRESERVED `SemaError` (never re-wrapped),
/// and a cancellation raises the structured `:cancelled` condition.
fn settlement_to_result(settlement: &TaskSettlement) -> NativeResult {
    match &settlement.outcome {
        TaskOutcome::Returned(value) => Ok(NativeOutcome::Return(value.clone())),
        TaskOutcome::Failed(error) => Err(error.clone()),
        TaskOutcome::Cancelled(_) => Err(cancelled_error()),
    }
}

/// Collect promise ids from a list/vector of promise handles, in input order.
fn collect_promise_ids(items: &[Value], name: &str) -> Result<Vec<PromiseId>, SemaError> {
    items
        .iter()
        .map(|item| expect_promise(std::slice::from_ref(item), name, 0))
        .collect()
}

/// Continuation that turns a runtime-allocated `PromiseId` (from `Spawn` or
/// `CreateSettledPromise`) into the language-facing promise handle. Holds no
/// `Value`, so its trace has no edges.
struct PromiseHandleCont;

impl Trace for PromiseHandleCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for PromiseHandleCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Promise(id)) => {
                Ok(NativeOutcome::Return(Value::async_promise_id(id)))
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            _ => Err(SemaError::eval(
                "async: promise creation returned an unexpected runtime response",
            )),
        }
    }
}

/// `async/await` continuation: the awaited promise settled (delivered as a
/// `Settlement`), or a failure/cancellation reached this frame.
struct AwaitCont;

impl Trace for AwaitCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for AwaitCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Settlement(Some(settlement))) => {
                settlement_to_result(&settlement)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            _ => Err(SemaError::eval(
                "async/await: awaited promise resumed with an unexpected runtime response",
            )),
        }
    }
}

/// `async/all` continuation: on success the runtime delivers every observed
/// settlement (input order) as `Settlements`; on short-circuit it delivers the
/// first failed/cancelled settlement as a single `Settlement`.
struct AllCont;

impl Trace for AllCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for AllCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Settlements(settlements)) => {
                let mut values = Vec::with_capacity(settlements.len());
                for settlement in &settlements {
                    match &settlement.outcome {
                        TaskOutcome::Returned(value) => values.push(value.clone()),
                        TaskOutcome::Failed(error) => return Err(error.clone()),
                        TaskOutcome::Cancelled(_) => return Err(cancelled_error()),
                    }
                }
                Ok(NativeOutcome::Return(Value::list(values)))
            }
            ResumeInput::Runtime(RuntimeResponse::Settlement(Some(settlement))) => {
                settlement_to_result(&settlement)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            _ => Err(SemaError::eval(
                "async/all: resumed with an unexpected runtime response",
            )),
        }
    }
}

/// `async/race` and `async/timeout` continuation: the winning settlement is
/// delivered as a single `Settlement`; an elapsed `async/timeout` deadline
/// arrives as `ResumeInput::Failed` (the structured `:timeout` condition).
struct RaceCont;

impl Trace for RaceCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for RaceCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Settlement(Some(settlement))) => {
                settlement_to_result(&settlement)
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            _ => Err(SemaError::eval(
                "async: combinator resumed with an unexpected runtime response",
            )),
        }
    }
}

/// `async/cancel` continuation: returns the boolean first-request result.
struct CancelCont;

impl Trace for CancelCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for CancelCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Cancelled(transitioned)) => {
                Ok(NativeOutcome::Return(Value::bool(transitioned)))
            }
            ResumeInput::Failed(error) => Err(error),
            _ => Err(SemaError::eval(
                "async/cancel: resumed with an unexpected runtime response",
            )),
        }
    }
}

/// Which terminal-state predicate an [`InspectCont`] reports.
#[derive(Clone, Copy)]
enum Predicate {
    Resolved,
    Rejected,
    Pending,
    Cancelled,
}

/// Continuation for the promise predicates: maps the inspected settlement
/// (`None` = still pending) to the predicate's boolean.
struct InspectCont {
    predicate: Predicate,
}

impl Trace for InspectCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for InspectCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Settlement(settlement)) => {
                let outcome = settlement.as_deref().map(|s| &s.outcome);
                let result = match self.predicate {
                    Predicate::Pending => outcome.is_none(),
                    Predicate::Resolved => matches!(outcome, Some(TaskOutcome::Returned(_))),
                    Predicate::Rejected => matches!(outcome, Some(TaskOutcome::Failed(_))),
                    Predicate::Cancelled => matches!(outcome, Some(TaskOutcome::Cancelled(_))),
                };
                Ok(NativeOutcome::Return(Value::bool(result)))
            }
            ResumeInput::Failed(error) => Err(error),
            _ => Err(SemaError::eval(
                "async: promise inspection returned an unexpected runtime response",
            )),
        }
    }
}

/// `async/run` continuation: the origin-root drain completed; resume with nil.
struct RunCont;

impl Trace for RunCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for RunCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            _ => Ok(NativeOutcome::Return(Value::nil())),
        }
    }
}

/// Build the `InspectPromise` request for a promise predicate.
fn inspect_op(args: &[Value], name: &str, predicate: Predicate) -> NativeResult {
    check_arity!(args, name, 1);
    let id = expect_promise(args, name, 0)?;
    Ok(NativeOutcome::Runtime(RuntimeRequest::InspectPromise {
        promise: id,
        continuation: Box::new(InspectCont { predicate }),
    }))
}

fn register_promise_ops(env: &Env) {
    // async/spawn — spawn a thunk as a detached task; resume with its promise.
    register_runtime_fn(env, "async/spawn", |args| {
        check_arity!(args, "async/spawn", 1);
        Ok(NativeOutcome::Runtime(RuntimeRequest::Spawn {
            callable: args[0].clone(),
            continuation: Box::new(PromiseHandleCont),
        }))
    });

    // async/await — suspend until the promise settles. A rejection re-raises the
    // PRESERVED error; a cancellation raises the structured `:cancelled`
    // condition. An already-settled promise resumes immediately (the runtime's
    // `install_promise_wait` delivers the settlement synchronously). Awaiting a
    // non-promise value is identity (like `await` of a non-thenable), so results
    // that are already plain values — e.g. an `async/all` value list — pass
    // straight through.
    register_runtime_fn(env, "async/await", |args| {
        check_arity!(args, "async/await", 1);
        match args[0].view() {
            ValueView::AsyncPromise(promise) => Ok(NativeOutcome::Suspend(NativeSuspend {
                wait: WaitKind::Promise(promise.id),
                continuation: Box::new(AwaitCont),
            })),
            _ => Ok(NativeOutcome::Return(args[0].clone())),
        }
    });

    // async/run — suspend on the self-resolving-waits barrier: wait until every
    // OTHER task in this task's origin-root graph has settled or come to rest on a
    // cycle-forming wait (see `Runtime::resolve_origin_barriers`), then resume nil.
    register_runtime_fn(env, "async/run", |args| {
        check_arity!(args, "async/run", 0);
        Ok(NativeOutcome::Runtime(RuntimeRequest::OriginBarrier {
            continuation: Box::new(RunCont),
        }))
    });

    // async/resolved — create an already-resolved promise.
    register_runtime_fn(env, "async/resolved", |args| {
        check_arity!(args, "async/resolved", 1);
        Ok(NativeOutcome::Runtime(
            RuntimeRequest::CreateSettledPromise {
                outcome: TaskOutcome::Returned(args[0].clone()),
                continuation: Box::new(PromiseHandleCont),
            },
        ))
    });

    // async/rejected — create an already-rejected promise. The reason string is
    // preserved as the promise's failure `SemaError`; awaiting it re-raises that
    // error verbatim (no `task rejected:` wrapping).
    register_runtime_fn(env, "async/rejected", |args| {
        check_arity!(args, "async/rejected", 1);
        let msg = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        Ok(NativeOutcome::Runtime(
            RuntimeRequest::CreateSettledPromise {
                outcome: TaskOutcome::Failed(SemaError::eval(msg)),
                continuation: Box::new(PromiseHandleCont),
            },
        ))
    });

    // Terminal-state predicates — inspect the registry settlement. The states
    // partition cleanly: a promise is at most one of resolved?/rejected?/
    // cancelled?, and pending? is exactly the not-yet-settled case.
    register_runtime_fn(env, "async/resolved?", |args| {
        inspect_op(args, "async/resolved?", Predicate::Resolved)
    });
    register_runtime_fn(env, "async/rejected?", |args| {
        inspect_op(args, "async/rejected?", Predicate::Rejected)
    });
    register_runtime_fn(env, "async/pending?", |args| {
        inspect_op(args, "async/pending?", Predicate::Pending)
    });
    register_runtime_fn(env, "async/cancelled?", |args| {
        inspect_op(args, "async/cancelled?", Predicate::Cancelled)
    });

    // async/cancel — request cancellation of the spawned task behind a promise.
    // Returns #t only when this call records the FIRST cancellation request for a
    // still-pending spawned task; #f for a synthetic/terminal/reaped promise.
    register_runtime_fn(env, "async/cancel", |args| {
        check_arity!(args, "async/cancel", 1);
        let id = expect_promise(args, "async/cancel", 0)?;
        Ok(NativeOutcome::Runtime(RuntimeRequest::CancelPromise {
            promise: id,
            continuation: Box::new(CancelCont),
        }))
    });

    // async/all — OBSERVE every supplied promise; resume with the input-ordered
    // value list once all resolve, or short-circuit on the first failure/cancel.
    // The supplied producers are never cancelled. Empty input resolves to `()`.
    register_runtime_fn(env, "async/all", |args| {
        check_arity!(args, "async/all", 1);
        let items = expect_list_or_vector(&args[0], "async/all")?;
        let promises = collect_promise_ids(items, "async/all")?;
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::PromiseSet(PromiseSetWait {
                promises,
                mode: PromiseSetMode::All,
            }),
            continuation: Box::new(AllCont),
        }))
    });

    // async/race — OBSERVE the supplied promises; resume with the first (lowest-
    // settlement) winner, returned/failed/cancelled alike. Losers CONTINUE.
    register_runtime_fn(env, "async/race", |args| {
        check_arity!(args, "async/race", 1);
        let items = expect_list_or_vector(&args[0], "async/race")?;
        if items.is_empty() {
            return Err(SemaError::eval("async/race: requires at least one promise"));
        }
        let promises = collect_promise_ids(items, "async/race")?;
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::PromiseSet(PromiseSetWait {
                promises,
                mode: PromiseSetMode::Race,
            }),
            continuation: Box::new(RaceCont),
        }))
    });

    // async/timeout — OBSERVE a single promise bounded by a deadline. An
    // already-settled promise wins (even at ms=0); a promise still pending at the
    // deadline raises `:timeout` while the supplied producer CONTINUES.
    register_runtime_fn(env, "async/timeout", |args| {
        check_arity!(args, "async/timeout", 2);
        let ms = duration_ms(&args[0], "async/timeout")?;
        const MAX_TIMEOUT_MS: i64 = 86_400_000; // 1 day
        if ms > MAX_TIMEOUT_MS {
            return Err(SemaError::eval(format!(
                "async/timeout: duration {ms} ms exceeds maximum {MAX_TIMEOUT_MS} ms (1 day)"
            ))
            .with_hint("split into smaller timeouts or remove the timeout entirely"));
        }
        let id = expect_promise(args, "async/timeout", 1)?;
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::PromiseSet(PromiseSetWait {
                promises: vec![id],
                mode: PromiseSetMode::Timeout(std::time::Duration::from_millis(ms as u64)),
            }),
            continuation: Box::new(RaceCont),
        }))
    });

    // async/sleep — suspend for a duration in milliseconds.
    //
    // Under the unified cooperative runtime (a `TaskContext` is installed) the
    // native suspends structurally on a timer wait; `SleepCont` resumes the
    // parked frame with nil when it fires. That runtime ABI is ALWAYS preferred
    // by `NativeFn::invoke_runtime` when present. The plain value ABI below is
    // for host-only synchronous callbacks and foreign synchronous VM re-entry.
    // If a synchronous helper bypasses `invoke_runtime` while a runtime quantum
    // is active, it cannot carry a suspension and fails explicitly.
    //
    // This suspend is unconditional (no promise-driven check): unlike the wasm
    // http natives, the SAME structural timer wait is correct on every
    // caller and target. Synchronous and promise-driven wasm entry points differ
    // only in how their drive loops wait out an `Idle` turn with a timer pending;
    // `drive_handle_to_settlement` owns that policy.
    env.set(
        sema_core::intern("async/sleep"),
        Value::native_fn(NativeFn::simple_with_runtime(
            "async/sleep",
            |args| {
                let ms = sleep_duration_ms(args)?;
                if in_runtime_quantum() {
                    return Err(SemaError::eval(
                        "async/sleep: entered through a synchronous callback path while the \
                         cooperative runtime is active — invoke it through a structural \
                         runtime call so it can suspend cleanly",
                    ));
                }
                // Outside any runtime quantum (a nested/foreign synchronous VM
                // re-entry), actually sleep. `ms` is unused on wasm32 (the main
                // thread must never block there).
                #[cfg(not(target_arch = "wasm32"))]
                std::thread::sleep(std::time::Duration::from_millis(ms));
                #[cfg(target_arch = "wasm32")]
                let _ = ms;
                Ok(Value::nil())
            },
            |_ctx, args| {
                let ms = sleep_duration_ms(args)?;
                Ok(NativeOutcome::Suspend(NativeSuspend {
                    wait: WaitKind::Timer(std::time::Duration::from_millis(ms)),
                    continuation: Box::new(SleepCont),
                }))
            },
        )),
    );
}

// ── Channel operations ───────────────────────────────────────────

/// The error raised when `channel/send` finds the channel closed (eagerly or
/// while a blocked sender is woken by a close). Names the dropped value so a
/// caller can see which send was lost.
fn channel_send_closed_error(value: &Value) -> SemaError {
    SemaError::eval(format!(
        "channel/send: channel is closed; value {value} was dropped"
    ))
}

/// Continuation turning a runtime-allocated `ChannelId` (from `CreateChannel`)
/// into the language-facing channel handle. Holds no `Value`.
struct ChannelHandleCont;

impl Trace for ChannelHandleCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ChannelHandleCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Channel(id)) => {
                Ok(NativeOutcome::Return(Value::channel_id(id)))
            }
            ResumeInput::Failed(error) => Err(error),
            _ => Err(SemaError::eval("channel/new: unexpected runtime response")),
        }
    }
}

/// `channel/send` continuation: `Sent` → nil; `Closed` → the closed-send error
/// naming the dropped value (carried here for the message — the runtime consumed
/// the sent copy).
struct SendCont {
    value: Value,
}

impl Trace for SendCont {
    fn trace(&self, sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        sink(GcEdge::Value(&self.value));
        true
    }
}

impl NativeContinuation for SendCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Send(ChannelSend::Sent)) => {
                Ok(NativeOutcome::Return(Value::nil()))
            }
            ResumeInput::Runtime(RuntimeResponse::Send(ChannelSend::Closed)) => {
                Err(channel_send_closed_error(&self.value))
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            _ => Err(SemaError::eval("channel/send: unexpected runtime response")),
        }
    }
}

/// `channel/recv` continuation: `Received(v)` → v; `Closed` → nil (the documented
/// closed-and-empty sentinel).
struct RecvCont;

impl Trace for RecvCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for RecvCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Receive(ChannelReceive::Received(value))) => {
                Ok(NativeOutcome::Return(value))
            }
            ResumeInput::Runtime(RuntimeResponse::Receive(ChannelReceive::Closed))
            | ResumeInput::Runtime(RuntimeResponse::Receive(ChannelReceive::Empty)) => {
                Ok(NativeOutcome::Return(Value::nil()))
            }
            ResumeInput::Failed(error) => Err(error),
            ResumeInput::Cancelled(_) => Err(cancelled_error()),
            _ => Err(SemaError::eval("channel/recv: unexpected runtime response")),
        }
    }
}

/// `channel/try-recv` continuation: `Received(v)` → v; `Empty`/`Closed` → nil.
struct TryRecvCont;

impl Trace for TryRecvCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for TryRecvCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Receive(ChannelReceive::Received(value))) => {
                Ok(NativeOutcome::Return(value))
            }
            ResumeInput::Runtime(RuntimeResponse::Receive(_)) => {
                Ok(NativeOutcome::Return(Value::nil()))
            }
            ResumeInput::Failed(error) => Err(error),
            _ => Err(SemaError::eval(
                "channel/try-recv: unexpected runtime response",
            )),
        }
    }
}

/// `channel/close` continuation: resume with nil regardless of whether this call
/// transitioned the channel (the boolean is internal).
struct CloseCont;

impl Trace for CloseCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for CloseCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Value(_)) => {
                Ok(NativeOutcome::Return(Value::nil()))
            }
            ResumeInput::Failed(error) => Err(error),
            _ => Err(SemaError::eval(
                "channel/close: unexpected runtime response",
            )),
        }
    }
}

/// Continuation for the observational channel ops (`count`/`empty?`/`full?`/
/// `closed?`): the runtime answers synchronously with the int/bool `Value`.
struct ChannelValueCont;

impl Trace for ChannelValueCont {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for ChannelValueCont {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        match input {
            ResumeInput::Runtime(RuntimeResponse::Value(value)) => Ok(NativeOutcome::Return(value)),
            ResumeInput::Failed(error) => Err(error),
            _ => Err(SemaError::eval("channel: unexpected runtime response")),
        }
    }
}

/// Build the `ChannelOp{Inspect(query)}` request for an observational channel op.
fn inspect_channel(args: &[Value], name: &str, query: ChannelQuery) -> NativeResult {
    check_arity!(args, name, 1);
    let channel = expect_channel(args, name, 0)?;
    Ok(NativeOutcome::Runtime(RuntimeRequest::ChannelOp {
        channel,
        operation: ChannelOperation::Inspect(query),
        continuation: Box::new(ChannelValueCont),
    }))
}

fn register_channel_ops(env: &Env) {
    // channel/new — create a bounded channel in the registry, resume with its
    // handle. Capacity validation stays here (the registry only allocates).
    register_runtime_fn(env, "channel/new", |args| {
        check_arity!(args, "channel/new", 0..=1);
        // An upper bound keeps an unrepresentable/allocation-impossible request
        // (e.g. `i64::MAX`) from reaching the registry's buffer, which would panic
        // on the capacity-overflow rather than returning a Sema condition.
        const MAX_CHANNEL_CAPACITY: usize = 1 << 24; // ~16M slots
        let capacity = if args.is_empty() {
            1
        } else {
            let n = args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
            if n <= 0 {
                return Err(SemaError::eval("channel/new: capacity must be at least 1"));
            }
            let cap = n as usize;
            if cap > MAX_CHANNEL_CAPACITY {
                return Err(SemaError::eval(format!(
                    "channel/new: capacity {n} exceeds maximum {MAX_CHANNEL_CAPACITY}"
                ))
                .with_hint("use a smaller bounded capacity"));
            }
            cap
        };
        Ok(NativeOutcome::Runtime(RuntimeRequest::CreateChannel {
            capacity,
            continuation: Box::new(ChannelHandleCont),
        }))
    });

    // channel/send — send a value; suspend until buffered/handed to a receiver.
    // A full channel BLOCKS until space (the runtime parks the frame); a
    // send-to-closed raises the closed error naming the dropped value.
    register_runtime_fn_with_escaping_args(env, "channel/send", &[1], |args| {
        check_arity!(args, "channel/send", 2);
        let channel = expect_channel(args, "channel/send", 0)?;
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::Channel(ChannelWait::Send {
                channel,
                value: args[1].clone(),
            }),
            continuation: Box::new(SendCont {
                value: args[1].clone(),
            }),
        }))
    });

    // channel/recv — receive a value; suspend until one is available. A closed +
    // empty channel resumes with nil (the documented closed sentinel).
    register_runtime_fn(env, "channel/recv", |args| {
        check_arity!(args, "channel/recv", 1);
        let channel = expect_channel(args, "channel/recv", 0)?;
        Ok(NativeOutcome::Suspend(NativeSuspend {
            wait: WaitKind::Channel(ChannelWait::Receive { channel }),
            continuation: Box::new(RecvCont),
        }))
    });

    // channel/try-recv — non-blocking receive: drain one value or nil sentinel.
    register_runtime_fn(env, "channel/try-recv", |args| {
        check_arity!(args, "channel/try-recv", 1);
        let channel = expect_channel(args, "channel/try-recv", 0)?;
        Ok(NativeOutcome::Runtime(RuntimeRequest::ChannelOp {
            channel,
            operation: ChannelOperation::TryReceive,
            continuation: Box::new(TryRecvCont),
        }))
    });

    // channel/close — close the channel, waking parked senders/receivers; nil.
    register_runtime_fn(env, "channel/close", |args| {
        check_arity!(args, "channel/close", 1);
        let channel = expect_channel(args, "channel/close", 0)?;
        Ok(NativeOutcome::Runtime(RuntimeRequest::ChannelOp {
            channel,
            operation: ChannelOperation::Close,
            continuation: Box::new(CloseCont),
        }))
    });

    // Observational channel ops — answered synchronously by the runtime.
    register_runtime_fn(env, "channel/closed?", |args| {
        inspect_channel(args, "channel/closed?", ChannelQuery::Closed)
    });
    register_runtime_fn(env, "channel/count", |args| {
        inspect_channel(args, "channel/count", ChannelQuery::Count)
    });
    register_runtime_fn(env, "channel/empty?", |args| {
        inspect_channel(args, "channel/empty?", ChannelQuery::Empty)
    });
    register_runtime_fn(env, "channel/full?", |args| {
        inspect_channel(args, "channel/full?", ChannelQuery::Full)
    });
}

#[cfg(test)]
mod duration_tests {
    use super::*;

    #[test]
    fn duration_ms_accepts_int_and_float_rejects_others() {
        assert_eq!(duration_ms(&Value::int(5), "t").unwrap(), 5); // int passes through
        assert_eq!(duration_ms(&Value::float(2.4), "t").unwrap(), 2); // float rounds down
        assert_eq!(duration_ms(&Value::float(2.6), "t").unwrap(), 3); // float rounds up
        assert_eq!(duration_ms(&Value::float(0.0), "t").unwrap(), 0);
        assert!(duration_ms(&Value::float(f64::INFINITY), "t").is_err()); // non-finite rejected
        assert!(duration_ms(&Value::float(f64::NAN), "t").is_err());
        assert!(duration_ms(&Value::string("nope"), "t").is_err()); // non-number rejected
    }
}

#[cfg(test)]
mod continuation_trace_tests {
    use super::*;

    fn edge_count(trace: &dyn Trace) -> usize {
        let mut count = 0;
        assert!(trace.trace(&mut |_| count += 1));
        count
    }

    /// Every promise-op continuation captures only `Copy` state (a `PromiseId`
    /// lives in the runtime request, never in the continuation; predicates hold a
    /// `Copy` enum). None holds a `Value`, so their GC trace must emit no edges —
    /// the CORE-2 invariant that keeps continuation state cycle-free.
    #[test]
    fn continuations_hold_no_value_edges() {
        assert_eq!(edge_count(&PromiseHandleCont), 0);
        assert_eq!(edge_count(&AwaitCont), 0);
        assert_eq!(edge_count(&AllCont), 0);
        assert_eq!(edge_count(&RaceCont), 0);
        assert_eq!(edge_count(&CancelCont), 0);
        assert_eq!(edge_count(&RunCont), 0);
        assert_eq!(
            edge_count(&InspectCont {
                predicate: Predicate::Resolved
            }),
            0
        );
    }

    /// The channel continuations are likewise edge-free EXCEPT `SendCont`, which
    /// carries the sent `Value` purely to name it in the closed-send error. That
    /// one `Value` must be a traced edge (CORE-2); the rest emit none.
    #[test]
    fn channel_continuations_trace_expected_edges() {
        assert_eq!(edge_count(&ChannelHandleCont), 0);
        assert_eq!(edge_count(&RecvCont), 0);
        assert_eq!(edge_count(&TryRecvCont), 0);
        assert_eq!(edge_count(&CloseCont), 0);
        assert_eq!(edge_count(&ChannelValueCont), 0);
        assert_eq!(
            edge_count(&SendCont {
                value: Value::int(7)
            }),
            1
        );
    }
}
