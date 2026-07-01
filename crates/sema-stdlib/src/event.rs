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
        if started.elapsed().as_millis() >= ms {
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

    // event/select — first ready source, or nil on timeout.
    register_fn(env, "event/select", |args| {
        check_arity!(args, "event/select", 1..=2);
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

        let started = Instant::now();
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
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::EvalContext;

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
}
