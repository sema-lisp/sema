//! A poll-based event selector for TUI loops (`event/select`, `time/tick`).
//!
//! The issue's motivating example is a coding agent that must react to
//! keyboard input, streaming subprocess output, and timer ticks in one loop
//! instead of "poll key, check signals, hope nothing is weird". `event/select`
//! takes a list of source descriptors and returns the first one that becomes
//! ready (or nil on timeout). It is poll-based (a short sleep between scans),
//! not edge-triggered — which is plenty for a human-paced TUI.
//!
//! Source descriptors (plain maps, so they're easy to build in Sema):
//!   {:type :key}                — a keypress is available on stdin
//!   {:type :proc  :handle <h>}  — a `proc/*` handle has output or has exited
//!   {:type :timer :ms <n>}      — n milliseconds have elapsed  (see time/tick)

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use sema_core::{check_arity, SemaError, Value};

use crate::register_fn;

/// Compute `(sources, timeout_ms, started)` for one `event/select` call — the
/// shared setup for the sync, legacy-async, and unified-runtime paths.
fn select_setup(args: &[Value]) -> Result<(Vec<Value>, u128, Instant), SemaError> {
    let sources = sources_of(&args[0])?;
    // Explicit timeout, else the smallest timer among the sources, else 10s.
    let explicit = args.get(1).and_then(|v| v.as_int());
    let min_timer = sources
        .iter()
        .filter_map(|s| s.as_map_ref())
        .filter(|m| m.get(&kw("type")) == Some(&kw("timer")))
        .filter_map(|m| m.get(&kw("ms")).and_then(|v| v.as_int()))
        .min();
    let timeout_ms = explicit.or(min_timer).unwrap_or(10_000).max(0) as u128;
    Ok((sources, timeout_ms, Instant::now()))
}

/// Value-ABI body for `event/select`: the sync polling loop plus the legacy
/// cooperative-scheduler `in_async_context()` `AwaitIo` poll. The unified-runtime
/// cooperative path lives in the runtime ABI (see [`register`]).
fn event_select_value(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "event/select", 1..=2);
    let (sources, timeout_ms, started) = select_setup(args)?;

    // In a legacy scheduler task, cooperatively poll the sources on the scheduler
    // thread rather than `std::thread::sleep`-blocking it between scans, so
    // sibling tasks (e.g. an LLM/agent task) make progress while we wait for a
    // source or the timeout. Reuses the same `AwaitIo` yield the file/http/shell
    // async paths use. The sync path below is unchanged.

    loop {
        for s in &sources {
            if let Some(ev) = ready(s, started) {
                return Ok(ev);
            }
        }
        if started.elapsed().as_millis() >= timeout_ms {
            return Ok(Value::nil());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The VM-thread readiness probe for `event/select` under the unified runtime:
/// the first ready source fires. Holds the source maps (live `Value`s), so it is
/// GC-traced — unlike the legacy untraced `AwaitIo` closure.
#[cfg(not(target_arch = "wasm32"))]
struct SourcesProbe {
    sources: Vec<Value>,
    started: Instant,
}

#[cfg(not(target_arch = "wasm32"))]
impl SourcesProbe {
    fn next_check_after_at(&self, now: Instant) -> Duration {
        let elapsed_ms = now.saturating_duration_since(self.started).as_millis();
        let next_timer = self
            .sources
            .iter()
            .filter_map(|source| {
                let map = source.as_map_ref()?;
                (map.get(&kw("type")) == Some(&kw("timer"))).then(|| {
                    let ms = map
                        .get(&kw("ms"))
                        .and_then(|value| value.as_int())
                        .unwrap_or(0)
                        .max(0) as u128;
                    Duration::from_millis(ms.saturating_sub(elapsed_ms) as u64)
                })
            })
            .min();

        let has_vm_probe = self.sources.iter().any(|source| {
            match source.as_map_ref().and_then(|map| map.get(&kw("type"))) {
                Some(kind) => kind != &kw("timer"),
                None => true,
            }
        });

        match (has_vm_probe, next_timer) {
            (false, Some(timer)) => timer,
            (true, Some(timer)) => timer.min(Duration::from_millis(5)),
            (true, None) | (false, None) => Duration::from_millis(5),
        }
    }

    fn poll_at(&self, now: Instant) -> crate::io::RuntimePollResult {
        if let Some(value) = self
            .sources
            .iter()
            .find_map(|source| ready_at(source, self.started, now))
        {
            return crate::io::RuntimePollResult::Ready(value);
        }
        crate::io::RuntimePollResult::PendingAfter(self.next_check_after_at(now))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl sema_core::runtime::Trace for SourcesProbe {
    fn trace(&self, sink: &mut dyn FnMut(sema_core::cycle::GcEdge<'_>)) -> bool {
        for s in &self.sources {
            sink(sema_core::cycle::GcEdge::Value(s));
        }
        true
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl crate::io::RuntimePoll for SourcesProbe {
    fn poll(&mut self) -> crate::io::RuntimePollResult {
        self.poll_at(Instant::now())
    }
}

fn kw(s: &str) -> Value {
    Value::keyword(s)
}

fn sources_of(v: &Value) -> Result<Vec<Value>, SemaError> {
    v.as_list()
        .or_else(|| v.as_vector())
        .map(|s| s.to_vec())
        .ok_or_else(|| SemaError::type_error("list of source maps", v.type_name()))
}

/// Build the event returned for a ready source: the source map plus details.
fn fire(source: &Value, extra: Vec<(&str, Value)>) -> Value {
    let mut m = BTreeMap::new();
    m.insert(kw("source"), source.clone());
    if let Some(t) = source.as_map_ref().and_then(|s| s.get(&kw("type"))) {
        m.insert(kw("type"), t.clone());
    }
    for (k, val) in extra {
        m.insert(kw(k), val);
    }
    Value::map(m)
}

/// Check one source; `Some(event)` if it's ready right now.
fn ready(source: &Value, started: Instant) -> Option<Value> {
    ready_at(source, started, Instant::now())
}

fn ready_at(source: &Value, started: Instant, now: Instant) -> Option<Value> {
    let m = source.as_map_ref()?;
    let ty = m.get(&kw("type"))?;
    if ty == &kw("key") {
        crate::io::poll_key_event(0).map(|v| fire(source, vec![("value", v)]))
    } else if ty == &kw("proc") {
        let h = m.get(&kw("handle"))?.as_int()?;
        match crate::proc::poll_ready(h) {
            Some((has_out, exited)) if has_out || exited => Some(fire(
                source,
                vec![
                    ("output?", Value::bool(has_out)),
                    ("exited?", Value::bool(exited)),
                ],
            )),
            _ => None,
        }
    } else if ty == &kw("timer") {
        let ms = m
            .get(&kw("ms"))
            .and_then(|v| v.as_int())
            .unwrap_or(0)
            .max(0) as u128;
        if now.saturating_duration_since(started).as_millis() >= ms {
            Some(fire(source, vec![]))
        } else {
            None
        }
    } else {
        None
    }
}

pub fn register(env: &sema_core::Env) {
    // time/tick — a reusable timer source for event/select: (time/tick 16).
    register_fn(env, "time/tick", |args| {
        check_arity!(args, "time/tick", 1);
        let ms = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        let mut m = BTreeMap::new();
        m.insert(kw("type"), kw("timer"));
        m.insert(kw("ms"), Value::int(ms.max(0)));
        Ok(Value::map(m))
    });

    // event/select — first ready source, or nil on timeout. Registered dual-ABI:
    // the value body serves the sync + legacy-scheduler paths; under the unified
    // runtime the runtime body yields a cooperative structural-timer poll so a TUI
    // "input OR agent progress" loop overlaps siblings without the legacy
    // `AwaitIo` bridge (crates/sema/tests/vm_async_test.rs asserts the yield).
    register_event_select(env);
}

#[cfg(not(target_arch = "wasm32"))]
fn register_event_select(env: &sema_core::Env) {
    use sema_core::runtime::NativeOutcome;
    env.set(
        sema_core::intern("event/select"),
        Value::native_fn(sema_core::NativeFn::simple_with_runtime(
            "event/select",
            event_select_value,
            |_ctx, args| {
                if sema_core::in_runtime_quantum() {
                    check_arity!(args, "event/select", 1..=2);
                    let (sources, timeout_ms, started) = select_setup(args)?;
                    return crate::io::await_runtime_until(
                        Box::new(SourcesProbe { sources, started }),
                        started,
                        timeout_ms as u64,
                    );
                }
                event_select_value(args).map(NativeOutcome::Return)
            },
        )),
    );
}

#[cfg(target_arch = "wasm32")]
fn register_event_select(env: &sema_core::Env) {
    register_fn(env, "event/select", event_select_value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::EvalContext;

    fn source(kind: &str, fields: &[(&str, Value)]) -> Value {
        let mut map = BTreeMap::new();
        map.insert(kw("type"), kw(kind));
        for (key, value) in fields {
            map.insert(kw(key), value.clone());
        }
        Value::map(map)
    }

    fn env() -> sema_core::Env {
        let e = sema_core::Env::new();
        register(&e);
        e
    }

    fn call(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let f = env.get_str(name).expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        (nf.func)(&EvalContext::default(), args).expect("call ok")
    }

    #[test]
    fn tick_builds_timer_source() {
        let e = env();
        let t = call(&e, "time/tick", &[Value::int(16)]);
        let m = t.as_map_ref().unwrap();
        assert_eq!(m.get(&kw("type")), Some(&kw("timer")));
        assert_eq!(m.get(&kw("ms")), Some(&Value::int(16)));
    }

    #[test]
    fn select_fires_timer() {
        let e = env();
        let tick = call(&e, "time/tick", &[Value::int(1)]);
        let ev = call(&e, "event/select", &[Value::list(vec![tick])]);
        let m = ev.as_map_ref().expect("an event, not nil");
        assert_eq!(m.get(&kw("type")), Some(&kw("timer")));
    }

    #[test]
    fn select_times_out_to_nil() {
        let e = env();
        // A proc source with a bogus handle never fires; 20ms timeout → nil.
        let mut src = BTreeMap::new();
        src.insert(kw("type"), kw("proc"));
        src.insert(kw("handle"), Value::int(999999));
        let ev = call(
            &e,
            "event/select",
            &[Value::list(vec![Value::map(src)]), Value::int(20)],
        );
        assert!(ev.is_nil());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sources_probe_uses_one_clock_sample_for_timer_readiness_and_rearm() {
        let started = Instant::now();
        let probe = SourcesProbe {
            sources: vec![
                source("timer", &[("ms", Value::int(100))]),
                source("timer", &[("ms", Value::int(25))]),
            ],
            started,
        };

        match probe.poll_at(started + Duration::from_millis(7)) {
            crate::io::RuntimePollResult::PendingAfter(delay) => {
                assert_eq!(delay, Duration::from_millis(18));
                assert_ne!(delay, Duration::ZERO);
            }
            other => panic!("expected a pending timer probe, got {other:?}"),
        }

        match probe.poll_at(started + Duration::from_millis(25)) {
            crate::io::RuntimePollResult::Ready(event) => {
                assert_eq!(
                    event.as_map_ref().unwrap().get(&kw("type")),
                    Some(&kw("timer"))
                );
            }
            other => panic!("expected the earliest timer to be ready, got {other:?}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn sources_probe_caps_vm_probe_delay() {
        let started = Instant::now();
        let probe = SourcesProbe {
            sources: vec![
                source("timer", &[("ms", Value::int(100))]),
                source("timer", &[("ms", Value::int(25))]),
                source("proc", &[("handle", Value::int(999999))]),
            ],
            started,
        };

        let delay = probe.next_check_after_at(started + Duration::from_millis(7));
        assert_eq!(delay, Duration::from_millis(5));
    }
}
