use std::collections::BTreeMap;

use sema_core::cycle::GcEdge;
use sema_core::runtime::{
    NativeCall, NativeCallContext, NativeContinuation, NativeOutcome, NativeResult, ResumeInput,
    Trace,
};
use sema_core::{check_arity, SemaError, Value};

use crate::register_fn;

/// Shared by `__retry-blocking` and `__retry-setup`: validate `thunk` is
/// callable and pull `{:max-attempts :base-delay-ms :backoff}` out of the
/// optional opts map, clamped/defaulted exactly as `(retry thunk opts)` has
/// always done. Kept in one place so the blocking native (sync path) and the
/// prelude-driven async loop (`__retry-setup`) can never drift apart.
fn retry_opts(args: &[Value]) -> Result<(u32, u64, f64), SemaError> {
    let thunk = &args[0];
    if thunk.as_lambda_rc().is_none() && thunk.as_native_fn_rc().is_none() {
        return Err(SemaError::type_error("function", thunk.type_name()));
    }

    let mut max_attempts: u32 = 3;
    let mut base_delay_ms: u64 = 100;
    let mut backoff: f64 = 2.0;

    if let Some(opts) = args.get(1).and_then(|v| v.as_map_rc()) {
        if let Some(v) = opts
            .get(&Value::keyword("max-attempts"))
            .and_then(|v| v.as_int())
        {
            max_attempts = v.max(1) as u32;
        }
        if let Some(v) = opts
            .get(&Value::keyword("base-delay-ms"))
            .and_then(|v| v.as_int())
        {
            base_delay_ms = v.max(0) as u64;
        }
        if let Some(v) = opts
            .get(&Value::keyword("backoff"))
            .and_then(|v| v.as_float())
        {
            backoff = v;
        }
    }

    Ok((max_attempts, base_delay_ms, backoff))
}

#[derive(Clone, Copy)]
enum TimingMode {
    ReturnValue,
    ReturnMillis,
}

impl TimingMode {
    fn name(self) -> &'static str {
        match self {
            Self::ReturnValue => "time",
            Self::ReturnMillis => "time/ms",
        }
    }
}

/// Complete a timing wrapper after its thunk settles. Timing state contains no
/// Sema values, so the continuation has no GC edges.
struct TimingContinuation {
    start: std::time::Instant,
    mode: TimingMode,
}

impl Trace for TimingContinuation {
    fn trace(&self, _sink: &mut dyn FnMut(GcEdge<'_>)) -> bool {
        true
    }
}

impl NativeContinuation for TimingContinuation {
    fn resume(
        self: Box<Self>,
        _context: &mut NativeCallContext<'_>,
        input: ResumeInput,
    ) -> NativeResult {
        let value = crate::list::resume_value(input, self.mode.name())?;
        let elapsed_ms = self.start.elapsed().as_secs_f64() * 1000.0;
        match self.mode {
            TimingMode::ReturnValue => {
                eprintln!("Elapsed: {elapsed_ms:.3}ms");
                Ok(NativeOutcome::Return(value))
            }
            TimingMode::ReturnMillis => Ok(NativeOutcome::Return(Value::float(elapsed_ms))),
        }
    }
}

fn timing_call(args: &[Value], mode: TimingMode) -> NativeResult {
    check_arity!(args, mode.name(), 1);
    Ok(NativeOutcome::Call(NativeCall {
        callable: args[0].clone(),
        args: Vec::new(),
        continuation: Box::new(TimingContinuation {
            start: std::time::Instant::now(),
            mode,
        }),
    }))
}

pub fn register(env: &sema_core::Env) {
    // __retry-blocking: the sync-path engine behind `(retry thunk)` / `(retry
    // thunk {:max-attempts 3 :base-delay-ms 100 :backoff 2.0})`. A native
    // cannot suspend and resume its own Rust for-loop across a scheduler
    // yield — the scheduler delivers a park's resume value straight to the
    // CALL SITE, not back into the middle of this loop (see
    // `sema_core::async_signal` / `docs/plans/archive/2026-07-02-
    // nonblocking-agent-run.md`) — so this real `thread::sleep` loop is
    // reserved for host/plain-worker callers. The prelude `retry` wrapper
    // routes runtime context through a Sema-level loop (`__retry-setup` below)
    // that backs off via `async/sleep` instead.
    register_fn(env, "__retry-blocking", |args| {
        check_arity!(args, "retry", 1..=2);
        let thunk = &args[0];
        let (max_attempts, base_delay_ms, backoff) = retry_opts(args)?;
        if sema_core::in_runtime_quantum() {
            return Err(SemaError::eval(
                "__retry-blocking is a host-only adapter; runtime code must use retry",
            ));
        }

        let mut last_error = None;
        for attempt in 0..max_attempts {
            match crate::list::call_function(thunk, &[]) {
                Ok(val) => return Ok(val),
                Err(e) => {
                    last_error = Some(e);
                    if attempt + 1 < max_attempts && base_delay_ms > 0 {
                        let delay = (base_delay_ms as f64 * backoff.powi(attempt as i32)) as u64;
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }
                }
            }
        }
        Err(last_error.unwrap())
    });

    // __retry-setup: validates `thunk`/arity and normalizes retry options
    // into a map the prelude `retry` wrapper's async-context Sema loop
    // consumes, via the exact same clamping/defaulting (and the same "retry"
    // arity error) as `__retry-blocking`.
    register_fn(env, "__retry-setup", |args| {
        check_arity!(args, "retry", 1..=2);
        let (max_attempts, base_delay_ms, backoff) = retry_opts(args)?;
        let mut m = BTreeMap::new();
        m.insert(
            Value::keyword("max-attempts"),
            Value::int(max_attempts as i64),
        );
        m.insert(
            Value::keyword("base-delay-ms"),
            Value::int(base_delay_ms as i64),
        );
        m.insert(Value::keyword("backoff"), Value::float(backoff));
        Ok(Value::map(m))
    });

    // (spy label value) — prints [label] value to stderr and returns value
    register_fn(env, "spy", |args| {
        check_arity!(args, "spy", 2);
        let label = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        eprintln!("[{}] {}", label, args[1]);
        Ok(args[1].clone())
    });

    // (time thunk) — calls zero-arg thunk, prints elapsed time to stderr, returns result
    crate::list::register_hof(
        env,
        "time",
        |args| {
            check_arity!(args, "time", 1);
            let start = std::time::Instant::now();
            let result = crate::list::call_function(&args[0], &[])?;
            let elapsed = start.elapsed();
            eprintln!("Elapsed: {:.3}ms", elapsed.as_secs_f64() * 1000.0);
            Ok(result)
        },
        |args| timing_call(args, TimingMode::ReturnValue),
    );

    // (time/ms thunk) — calls zero-arg thunk, returns elapsed time in ms as float
    crate::list::register_hof(
        env,
        "time/ms",
        |args| {
            check_arity!(args, "time/ms", 1);
            let start = std::time::Instant::now();
            let _result = crate::list::call_function(&args[0], &[])?;
            let elapsed = start.elapsed();
            Ok(Value::float(elapsed.as_secs_f64() * 1000.0))
        },
        |args| timing_call(args, TimingMode::ReturnMillis),
    );

    // (assert condition) or (assert condition message) — throws if condition is falsy
    register_fn(env, "assert", |args| {
        if args.is_empty() || args.len() > 2 {
            return Err(SemaError::arity("assert", "1-2", args.len()));
        }
        if !args[0].is_truthy() {
            let msg = if args.len() == 2 {
                if let Some(s) = args[1].as_str() {
                    s.to_string()
                } else {
                    args[1].to_string()
                }
            } else {
                "assertion failed".to_string()
            };
            return Err(SemaError::eval(msg));
        }
        Ok(Value::bool(true))
    });

    // (assert= expected actual) — throws if not equal, with a diff message
    register_fn(env, "assert=", |args| {
        check_arity!(args, "assert=", 2);
        if args[0] != args[1] {
            return Err(SemaError::eval(format!(
                "assertion failed: expected {}, got {}",
                args[0], args[1]
            )));
        }
        Ok(Value::bool(true))
    });

    register_fn(env, "gensym", |args| {
        check_arity!(args, "gensym", 0..=1);
        let prefix = if args.len() == 1 {
            args[0]
                .as_str()
                .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?
                .to_string()
        } else {
            "g".to_string()
        };
        Ok(Value::symbol(&sema_core::next_gensym(&prefix)))
    });
}
