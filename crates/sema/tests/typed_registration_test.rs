//! Acceptance checks for typed registration and the public value view (#179),
//! through the public `sema` crate only.

#![cfg(not(target_arch = "wasm32"))]

use std::convert::Infallible;
use std::time::Duration;

use sema::{Interpreter, Keyword, SemaError, Value, ValueRef};

fn type_of_error(interp: &Interpreter, code: &str) -> Value {
    interp
        .eval_str(&format!("(try {code} (catch e (:type e)))"))
        .expect("error is catchable")
}

#[test]
fn sync_typed_functions_convert_arguments_and_results() {
    let interp = Interpreter::new();
    interp.register_fn_typed("host/zero", || Ok::<_, SemaError>(42_i64));
    interp.register_fn_typed("host/add", |a: i64, b: i64| Ok::<_, SemaError>(a + b));
    interp.register_fn_typed("host/sum", |xs: Vec<f64>| {
        Ok::<_, SemaError>(xs.iter().sum::<f64>())
    });
    interp.register_fn_typed("host/greet", |name: Option<String>| {
        Ok::<_, SemaError>(format!("hello, {}", name.as_deref().unwrap_or("world")))
    });
    interp.register_fn_typed("host/tag", |k: Keyword| {
        Ok::<_, SemaError>(Keyword(format!("{}-seen", k.0)))
    });

    assert_eq!(interp.eval_str("(host/zero)").unwrap(), Value::int(42));
    assert_eq!(interp.eval_str("(host/add 2 3)").unwrap(), Value::int(5));
    assert_eq!(
        interp.eval_str("(host/sum [1 2.5])").unwrap(),
        Value::float(3.5)
    );
    assert_eq!(
        interp.eval_str("(host/greet nil)").unwrap(),
        Value::string("hello, world")
    );
    assert_eq!(
        interp.eval_str("(host/greet \"kameko\")").unwrap(),
        Value::string("hello, kameko")
    );
    assert_eq!(
        interp.eval_str("(host/tag :x)").unwrap(),
        Value::keyword("x-seen")
    );
}

#[test]
fn typed_errors_match_manual_register_fn_classification() {
    let interp = Interpreter::new();
    interp.register_fn_typed("host/add", |a: i64, b: i64| Ok::<_, SemaError>(a + b));
    interp.register_fn("host/add-manual", |args| {
        if args.len() != 2 {
            return Err(SemaError::arity("host/add-manual", "2", args.len()));
        }
        let a = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[0].type_name()))?;
        let b = args[1]
            .as_int()
            .ok_or_else(|| SemaError::type_error("int", args[1].type_name()))?;
        Ok(Value::int(a + b))
    });

    for (typed, manual) in [
        ("(host/add 1)", "(host/add-manual 1)"),
        ("(host/add 1 \"x\")", "(host/add-manual 1 \"x\")"),
    ] {
        assert_eq!(
            type_of_error(&interp, typed),
            type_of_error(&interp, manual),
            "{typed} vs {manual}"
        );
    }
    assert_eq!(
        type_of_error(&interp, "(host/add 1)"),
        Value::keyword("arity")
    );

    let error = interp.eval_str("(host/add 1 \"x\")").unwrap_err();
    assert!(matches!(error.inner(), SemaError::Type { .. }), "{error}");
    assert!(
        format!("{error:?}").contains("argument 2 of host/add"),
        "position is noted: {error:?}"
    );
}

#[test]
fn closure_errors_propagate_unchanged() {
    let interp = Interpreter::new();
    interp.register_fn_typed("host/positive", |n: i64| {
        if n < 0 {
            Err(SemaError::eval("must be non-negative"))
        } else {
            Ok(n)
        }
    });
    let error = interp.eval_str("(host/positive -1)").unwrap_err();
    assert!(
        error.to_string().contains("must be non-negative"),
        "{error}"
    );
}

#[test]
fn async_typed_functions_suspend_and_convert() {
    let interp = Interpreter::new();
    interp.register_async_fn_typed("host/delayed-upper", |text: String, ms: u64| async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok::<_, Infallible>(text.to_uppercase())
    });
    interp.register_async_fn_typed("host/ids", |n: usize| async move {
        Ok::<_, Infallible>((0..n as i64).collect::<Vec<i64>>())
    });
    assert_eq!(
        interp.eval_str("(host/delayed-upper \"hi\" 5)").unwrap(),
        Value::string("HI")
    );
    assert_eq!(
        interp.eval_str("(host/ids 3)").unwrap().to_string(),
        "(0 1 2)"
    );
    // Two calls overlap from spawned tasks.
    assert_eq!(
        interp
            .eval_str(
                "(async/all (list (async (host/delayed-upper \"a\" 20)) \
                                  (async (host/delayed-upper \"b\" 20))))"
            )
            .unwrap()
            .to_string(),
        "(\"A\" \"B\")"
    );
    assert_eq!(
        type_of_error(&interp, "(host/ids \"x\")"),
        Value::keyword("type-error")
    );
    assert_eq!(
        type_of_error(&interp, "(host/ids)"),
        Value::keyword("arity")
    );
    assert_eq!(interp.runtime_live_task_count(), 0);
}

#[test]
fn value_ref_matches_host_arguments() {
    let interp = Interpreter::new();
    interp.register_fn("host/describe", |args| {
        let text = match ValueRef::slice(args).as_slice() {
            [] => "none".to_string(),
            [ValueRef::Keyword(k), ValueRef::Int(n)] if *k == "retry" => format!("retry {n}"),
            [ValueRef::Keyword(k), ValueRef::Int(n)] => format!("{k}={n}"),
            [ValueRef::Map(m)] => format!(
                "map with {} entries, status {:?}",
                m.len(),
                m.get_keyword("status").and_then(Value::as_int)
            ),
            [ValueRef::List(items)] | [ValueRef::Vector(items)] => format!("{} items", items.len()),
            [other @ ValueRef::Other(_)] => format!("other {}", other.type_name()),
            other => format!("{} args", other.len()),
        };
        Ok(Value::string(&text))
    });
    for (code, expected) in [
        ("(host/describe)", "none"),
        ("(host/describe :retry 3)", "retry 3"),
        ("(host/describe :limit 9)", "limit=9"),
        (
            "(host/describe {:status 200 :ok true})",
            "map with 2 entries, status Some(200)",
        ),
        ("(host/describe [1 2 3])", "3 items"),
        ("(host/describe (fn () 1))", "other lambda"),
    ] {
        assert_eq!(
            interp.eval_str(code).unwrap(),
            Value::string(expected),
            "{code}"
        );
    }
}
