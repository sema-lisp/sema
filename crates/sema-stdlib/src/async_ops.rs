use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use sema_core::{
    call_run_scheduler, call_run_scheduler_all_of, call_run_scheduler_any_of,
    call_run_scheduler_timeout, call_spawn_callback, check_arity, in_async_context,
    set_debug_coop_resume, set_yield_signal, take_resume_value, AsyncPromise, Channel,
    DebugCoopResume, Env, EvalContext, NativeFn, PromiseState, SchedulerRunResult, SchedulerTarget,
    SemaError, Value, ValueView, YieldReason,
};

use crate::register_fn;

/// Format a normal task rejection as an `async/await` error, stripping
/// any already-present `async/await: task rejected:` prefix so that
/// chained awaits don't quadratically nest the prefix.
fn rejected_error(e: &str) -> SemaError {
    let core = e
        .strip_prefix("Eval error: async/await: task rejected: ")
        .or_else(|| e.strip_prefix("async/await: task rejected: "))
        .unwrap_or(e);
    SemaError::eval(format!("async/await: task rejected: {core}"))
}

/// Format the `await`-on-cancelled-promise error. Distinct from a
/// normal rejection so users can branch on `:type :cancelled` once
/// we expose that.
fn cancelled_error() -> SemaError {
    SemaError::eval("async/await: task was cancelled")
        .with_hint("the task was cancelled via async/cancel before it produced a value")
}

// ── Helpers ──────────────────────────────────────────────────────

fn register_fn_ctx(
    env: &Env,
    name: &str,
    f: impl Fn(&EvalContext, &[Value]) -> Result<Value, SemaError> + 'static,
) {
    env.set(
        sema_core::intern(name),
        Value::native_fn(NativeFn::with_ctx(name, f)),
    );
}

fn expect_promise(args: &[Value], _name: &str, idx: usize) -> Result<Rc<AsyncPromise>, SemaError> {
    match args[idx].view() {
        ValueView::AsyncPromise(p) => Ok(p),
        _ => Err(SemaError::type_error_with_value(
            "async-promise",
            args[idx].type_name(),
            &args[idx],
        )),
    }
}

fn expect_channel(args: &[Value], _name: &str, idx: usize) -> Result<Rc<Channel>, SemaError> {
    match args[idx].view() {
        ValueView::Channel(c) => Ok(c),
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

fn register_promise_ops(env: &Env) {
    // async/spawn — spawn a thunk as an async task, returns a promise
    register_fn_ctx(env, "async/spawn", |ctx, args| {
        check_arity!(args, "async/spawn", 1);
        call_spawn_callback(ctx, args[0].clone())
    });

    // async/await — wait for a promise to resolve
    register_fn_ctx(env, "async/await", |ctx, args| {
        check_arity!(args, "async/await", 1);
        let promise = expect_promise(args, "async/await", 0)?;

        // Check for resume value first (we're resuming from a yield)
        if let Some(val) = take_resume_value() {
            return Ok(val);
        }

        // If already resolved, return immediately
        {
            let state = promise.state.borrow();
            match &*state {
                PromiseState::Resolved(v) => return Ok(v.clone()),
                PromiseState::Rejected(e) => return Err(rejected_error(e)),
                PromiseState::Cancelled => return Err(cancelled_error()),
                PromiseState::Pending => {}
            }
        }

        // If in async context, yield
        if in_async_context() {
            set_yield_signal(YieldReason::AwaitPromise(promise));
            return Ok(Value::nil()); // placeholder, VM catches the signal
        }

        // At top level, run the scheduler inline.
        if call_run_scheduler(ctx, Some(promise.clone()))? == SchedulerRunResult::DebugPaused {
            // A breakpoint fired inside a task during a cooperative (WASM) debug
            // session: the target is still pending. Yield the main VM so
            // `run_cooperative` surfaces the stop to JS, and record how to resume
            // (re-drive the scheduler, then return this promise's value). The
            // native re-runs on resume via `take_resume_value` above.
            set_debug_coop_resume(
                SchedulerTarget::One(promise.clone()),
                DebugCoopResume::Await(promise.clone()),
            );
            set_yield_signal(YieldReason::AwaitPromise(promise));
            return Ok(Value::nil());
        }
        let state = promise.state.borrow();
        match &*state {
            PromiseState::Resolved(v) => Ok(v.clone()),
            PromiseState::Rejected(e) => Err(rejected_error(e)),
            PromiseState::Cancelled => Err(cancelled_error()),
            PromiseState::Pending => Err(SemaError::eval(
                "async/await: still pending after scheduler run",
            )),
        }
    });

    // async/run — run all pending tasks to completion
    register_fn_ctx(env, "async/run", |ctx, args| {
        check_arity!(args, "async/run", 0);
        if call_run_scheduler(ctx, None)? == SchedulerRunResult::DebugPaused {
            set_debug_coop_resume(SchedulerTarget::All, DebugCoopResume::Run);
            // No specific promise to await: park on the scheduler re-drive via a
            // dummy never-resolving signal is wrong, so yield with an All target
            // surrogate. `run_cooperative` re-drives `SchedulerTarget::All`.
            set_yield_signal(YieldReason::Sleep(0));
            return Ok(Value::nil());
        }
        Ok(Value::nil())
    });

    // async/resolved — create an already-resolved promise
    register_fn(env, "async/resolved", |args| {
        check_arity!(args, "async/resolved", 1);
        Ok(Value::async_promise(AsyncPromise {
            state: RefCell::new(PromiseState::Resolved(args[0].clone())),
            task_id: Cell::new(0),
        }))
    });

    // async/rejected — create an already-rejected promise
    register_fn(env, "async/rejected", |args| {
        check_arity!(args, "async/rejected", 1);
        let msg = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
            .to_string();
        Ok(Value::async_promise(AsyncPromise {
            state: RefCell::new(PromiseState::Rejected(msg)),
            task_id: Cell::new(0),
        }))
    });

    // async/resolved? — check if promise is resolved
    register_fn(env, "async/resolved?", |args| {
        check_arity!(args, "async/resolved?", 1);
        let promise = expect_promise(args, "async/resolved?", 0)?;
        let resolved = matches!(&*promise.state.borrow(), PromiseState::Resolved(_));
        Ok(Value::bool(resolved))
    });

    // async/rejected? — true exactly when the promise is in the Rejected state.
    // Excludes Cancelled (which is its own peer variant) so the predicates
    // partition the terminal states cleanly: a promise is at most one of
    // resolved? / rejected? / cancelled?.
    register_fn(env, "async/rejected?", |args| {
        check_arity!(args, "async/rejected?", 1);
        let promise = expect_promise(args, "async/rejected?", 0)?;
        let rejected = matches!(&*promise.state.borrow(), PromiseState::Rejected(_));
        Ok(Value::bool(rejected))
    });

    // async/pending? — check if promise is still pending
    register_fn(env, "async/pending?", |args| {
        check_arity!(args, "async/pending?", 1);
        let promise = expect_promise(args, "async/pending?", 0)?;
        let pending = matches!(&*promise.state.borrow(), PromiseState::Pending);
        Ok(Value::bool(pending))
    });

    // async/cancel — request cancellation of a spawned async task.
    // Returns #t if this call actually transitioned the promise into
    // Cancelled, #f if the promise was already terminal (resolved,
    // rejected, or previously cancelled) or was never spawned in the
    // first place (e.g. created via async/resolved). Never errors on
    // a non-spawned promise — cancellation is best-effort.
    register_fn(env, "async/cancel", |args| {
        check_arity!(args, "async/cancel", 1);
        let promise = expect_promise(args, "async/cancel", 0)?;
        let task_id = promise.task_id.get();
        if task_id == 0 {
            // Never-spawned promise (async/resolved / async/rejected).
            // There's nothing to cancel; report no transition.
            return Ok(Value::bool(false));
        }
        let transitioned = sema_core::call_cancel_callback(task_id)?;
        Ok(Value::bool(transitioned))
    });

    // async/cancelled? — true exactly when the promise is in the Cancelled state.
    // Distinct from async/rejected? — a cancelled promise is not a normal
    // rejection (which a user might catch and recover from). Matches the
    // PromiseState::Cancelled variant directly so a user manually rejecting
    // with the string "cancelled" no longer fools this predicate.
    register_fn(env, "async/cancelled?", |args| {
        check_arity!(args, "async/cancelled?", 1);
        let promise = expect_promise(args, "async/cancelled?", 0)?;
        let is_cancelled = matches!(&*promise.state.borrow(), PromiseState::Cancelled);
        Ok(Value::bool(is_cancelled))
    });

    // async/all — run scheduler and collect results from all promises
    register_fn_ctx(env, "async/all", |ctx, args| {
        check_arity!(args, "async/all", 1);
        let items = expect_list_or_vector(&args[0], "async/all")?;

        let promises: Vec<Rc<AsyncPromise>> = items
            .iter()
            .map(|item| expect_promise(std::slice::from_ref(item), "async/all", 0))
            .collect::<Result<_, _>>()?;

        // Run scheduler until the requested promises settle. Unrelated
        // background tasks must not make this combinator report deadlock.
        if call_run_scheduler_all_of(ctx, promises.clone())? == SchedulerRunResult::DebugPaused {
            // Cooperative debug pause inside a task: yield the main VM and re-run
            // this native on resume (it re-collects the now-settled promises).
            set_debug_coop_resume(
                SchedulerTarget::AllOf(promises.clone()),
                DebugCoopResume::All(promises.clone()),
            );
            // Yield against the first still-pending promise so the VM suspends.
            let pending = promises
                .iter()
                .find(|p| matches!(&*p.state.borrow(), PromiseState::Pending))
                .cloned()
                .unwrap_or_else(|| promises[0].clone());
            set_yield_signal(YieldReason::AwaitPromise(pending));
            return Ok(Value::nil());
        }

        // Collect results — propagate the first non-resolved settlement.
        // Report a REJECTION (the cause) in preference to a Cancelled sibling: on a
        // rejection short-circuit the scheduler transitively cancels the still-pending
        // siblings (ASYNC-3), so a cancelled task here is usually a *consequence* of
        // another task's rejection — surfacing it would mask the real failure reason.
        let mut results = Vec::with_capacity(items.len());
        for p in &promises {
            if let PromiseState::Rejected(e) = &*p.state.borrow() {
                return Err(SemaError::eval(format!("async/all: task rejected: {e}")));
            }
        }
        for p in &promises {
            if matches!(&*p.state.borrow(), PromiseState::Cancelled) {
                return Err(SemaError::eval("async/all: task was cancelled"));
            }
        }
        for p in promises {
            let state = p.state.borrow();
            match &*state {
                PromiseState::Resolved(v) => results.push(v.clone()),
                PromiseState::Rejected(_) | PromiseState::Cancelled => {
                    unreachable!("non-resolved states handled above")
                }
                PromiseState::Pending => {
                    return Err(SemaError::eval("async/all: task still pending"))
                }
            }
        }
        Ok(Value::list(results))
    });

    // async/race — run scheduler and return the first resolved promise's value
    register_fn_ctx(env, "async/race", |ctx, args| {
        check_arity!(args, "async/race", 1);
        let items = expect_list_or_vector(&args[0], "async/race")?;

        if items.is_empty() {
            return Err(SemaError::eval("async/race: requires at least one promise"));
        }

        // Collect promises
        let promises: Vec<Rc<AsyncPromise>> = items
            .iter()
            .map(|item| expect_promise(std::slice::from_ref(item), "async/race", 0))
            .collect::<Result<_, _>>()?;

        // Check if any already resolved
        for p in &promises {
            if let PromiseState::Resolved(v) = &*p.state.borrow() {
                return Ok(v.clone());
            }
        }

        // Run scheduler until one requested promise settles. Unrelated
        // background tasks must not make this combinator report deadlock.
        if call_run_scheduler_any_of(ctx, promises.clone())? == SchedulerRunResult::DebugPaused {
            set_debug_coop_resume(
                SchedulerTarget::AnyOf(promises.clone()),
                DebugCoopResume::Race(promises.clone()),
            );
            let pending = promises
                .iter()
                .find(|p| matches!(&*p.state.borrow(), PromiseState::Pending))
                .cloned()
                .unwrap_or_else(|| promises[0].clone());
            set_yield_signal(YieldReason::AwaitPromise(pending));
            return Ok(Value::nil());
        }

        // Find first resolved
        for p in &promises {
            if let PromiseState::Resolved(v) = &*p.state.borrow() {
                return Ok(v.clone());
            }
        }

        // Check for rejections
        for p in &promises {
            if let PromiseState::Rejected(e) = &*p.state.borrow() {
                return Err(SemaError::eval(format!("async/race: task rejected: {e}")));
            }
        }

        Err(SemaError::eval("async/race: no promise resolved"))
    });

    // async/timeout — race a promise against a deadline
    register_fn_ctx(env, "async/timeout", |ctx, args| {
        check_arity!(args, "async/timeout", 2);
        let ms = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        if ms < 0 {
            return Err(SemaError::eval(
                "async/timeout: duration must be non-negative",
            ));
        }
        const MAX_TIMEOUT_MS: i64 = 86_400_000; // 1 day
        if ms > MAX_TIMEOUT_MS {
            return Err(SemaError::eval(format!(
                "async/timeout: duration {ms} ms exceeds maximum {MAX_TIMEOUT_MS} ms (1 day)"
            ))
            .with_hint("split into smaller timeouts or remove the timeout entirely"));
        }
        let promise = expect_promise(args, "async/timeout", 1)?;

        // If already resolved/rejected, return immediately
        {
            let state = promise.state.borrow();
            match &*state {
                PromiseState::Resolved(v) => return Ok(v.clone()),
                PromiseState::Rejected(e) => {
                    return Err(SemaError::eval(format!(
                        "async/timeout: task rejected: {e}"
                    )))
                }
                PromiseState::Cancelled => {
                    return Err(SemaError::eval("async/timeout: task was cancelled"))
                }
                PromiseState::Pending => {}
            }
        }

        // Run scheduler until the promise resolves or the timeout elapses.
        match call_run_scheduler_timeout(ctx, promise.clone(), ms as u64)? {
            SchedulerRunResult::TimedOut => {
                return Err(SemaError::eval("async/timeout: operation timed out"));
            }
            SchedulerRunResult::DebugPaused => {
                // Cooperative debug pause inside the awaited task: yield + re-run.
                set_debug_coop_resume(
                    SchedulerTarget::One(promise.clone()),
                    DebugCoopResume::Await(promise.clone()),
                );
                set_yield_signal(YieldReason::AwaitPromise(promise));
                return Ok(Value::nil());
            }
            SchedulerRunResult::Complete => {}
        }

        // Check if resolved
        {
            let state = promise.state.borrow();
            match &*state {
                PromiseState::Resolved(v) => return Ok(v.clone()),
                PromiseState::Rejected(e) => {
                    return Err(SemaError::eval(format!(
                        "async/timeout: task rejected: {e}"
                    )))
                }
                PromiseState::Cancelled => {
                    return Err(SemaError::eval("async/timeout: task was cancelled"))
                }
                PromiseState::Pending => {}
            }
        }

        Err(SemaError::eval(
            "async/timeout: operation is still pending after scheduler run",
        ))
    });

    // async/sleep — yield for a duration in milliseconds
    register_fn(env, "async/sleep", |args| {
        check_arity!(args, "async/sleep", 1);
        let ms = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        if ms < 0 {
            return Err(SemaError::eval(
                "async/sleep: duration must be non-negative",
            ));
        }
        // Cap the duration (mirrors async/timeout). The scheduler's virtual
        // clock jumps straight to a sleeper's wake time and, on native, waits
        // that whole delta in one `thread::sleep`; without a bound an
        // out-of-range duration would wedge the thread for years and could
        // overflow the virtual clock.
        const MAX_SLEEP_MS: i64 = 86_400_000; // 1 day
        if ms > MAX_SLEEP_MS {
            return Err(SemaError::eval(format!(
                "async/sleep: duration {ms} ms exceeds maximum {MAX_SLEEP_MS} ms (1 day)"
            ))
            .with_hint("use a shorter sleep, or loop with smaller sleeps"));
        }
        if in_async_context() {
            if let Some(cached) = take_resume_value() {
                return Ok(cached);
            }
            set_yield_signal(YieldReason::Sleep(ms as u64));
            return Ok(Value::nil());
        }
        // Outside async, actually sleep
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(std::time::Duration::from_millis(ms as u64));
        Ok(Value::nil())
    });
}

// ── Channel operations ───────────────────────────────────────────

fn register_channel_ops(env: &Env) {
    // channel/new — create a bounded channel
    register_fn(env, "channel/new", |args| {
        check_arity!(args, "channel/new", 0..=1);
        let capacity = if args.is_empty() {
            1
        } else {
            let n = args[0]
                .as_int()
                .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
            if n <= 0 {
                return Err(SemaError::eval("channel/new: capacity must be at least 1"));
            }
            n as usize
        };
        Ok(Value::channel(Channel {
            buffer: RefCell::new(VecDeque::with_capacity(capacity)),
            capacity,
            closed: Cell::new(false),
        }))
    });

    // channel/send — send a value to a channel (yields if full in async context)
    register_fn(env, "channel/send", |args| {
        check_arity!(args, "channel/send", 2);
        let ch = expect_channel(args, "channel/send", 0)?;
        if ch.closed.get() {
            return Err(SemaError::eval(format!(
                "channel/send: channel is closed; value {} was dropped",
                args[1]
            )));
        }
        if in_async_context() {
            if let Some(cached) = take_resume_value() {
                return Ok(cached);
            }
        }
        let mut buf = ch.buffer.borrow_mut();
        if buf.len() >= ch.capacity {
            drop(buf);
            if in_async_context() {
                set_yield_signal(YieldReason::ChannelSend(ch, args[1].clone()));
                return Ok(Value::nil());
            }
            return Err(
                SemaError::eval("channel/send: channel is full").with_hint(
                    "Use async to run in an async context where send will yield until space is available",
                ),
            );
        }
        buf.push_back(args[1].clone());
        Ok(Value::nil())
    });

    // channel/recv — receive a value from a channel (yields if empty in async context)
    register_fn(env, "channel/recv", |args| {
        check_arity!(args, "channel/recv", 1);
        let ch = expect_channel(args, "channel/recv", 0)?;
        if in_async_context() {
            if let Some(cached) = take_resume_value() {
                return Ok(cached);
            }
        }
        let mut buf = ch.buffer.borrow_mut();
        if let Some(v) = buf.pop_front() {
            return Ok(v);
        }
        drop(buf);
        if ch.closed.get() {
            return Ok(Value::nil());
        }
        if in_async_context() {
            set_yield_signal(YieldReason::ChannelRecv(ch));
            return Ok(Value::nil());
        }
        Err(SemaError::eval("channel/recv: channel is empty"))
    });

    // channel/try-recv — non-blocking receive (returns nil if empty)
    register_fn(env, "channel/try-recv", |args| {
        check_arity!(args, "channel/try-recv", 1);
        let ch = expect_channel(args, "channel/try-recv", 0)?;
        let val = ch.buffer.borrow_mut().pop_front().unwrap_or(Value::nil());
        Ok(val)
    });

    // channel/close — close a channel
    register_fn(env, "channel/close", |args| {
        check_arity!(args, "channel/close", 1);
        let ch = expect_channel(args, "channel/close", 0)?;
        ch.closed.set(true);
        Ok(Value::nil())
    });

    // channel/closed? — check if a channel is closed
    register_fn(env, "channel/closed?", |args| {
        check_arity!(args, "channel/closed?", 1);
        let ch = expect_channel(args, "channel/closed?", 0)?;
        Ok(Value::bool(ch.closed.get()))
    });

    // channel/count — number of items currently in the buffer
    register_fn(env, "channel/count", |args| {
        check_arity!(args, "channel/count", 1);
        let ch = expect_channel(args, "channel/count", 0)?;
        let len = ch.buffer.borrow().len();
        Ok(Value::int(len as i64))
    });

    // channel/empty? — check if the channel buffer is empty
    register_fn(env, "channel/empty?", |args| {
        check_arity!(args, "channel/empty?", 1);
        let ch = expect_channel(args, "channel/empty?", 0)?;
        let empty = ch.buffer.borrow().is_empty();
        Ok(Value::bool(empty))
    });

    // channel/full? — check if the channel buffer is at capacity
    register_fn(env, "channel/full?", |args| {
        check_arity!(args, "channel/full?", 1);
        let ch = expect_channel(args, "channel/full?", 0)?;
        let buf = ch.buffer.borrow();
        Ok(Value::bool(buf.len() >= ch.capacity))
    });
}
