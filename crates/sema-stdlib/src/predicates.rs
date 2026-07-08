use sema_core::number::SemaNumber;
use sema_core::{check_arity, SemaError, Value, ValueViewRef};

use crate::register_fn;

fn bool_pred(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "bool?", 1);
    Ok(Value::bool(args[0].as_bool().is_some()))
}

fn procedure_pred(args: &[Value]) -> Result<Value, SemaError> {
    check_arity!(args, "fn?", 1);
    Ok(Value::bool(
        args[0].as_lambda_rc().is_some() || args[0].as_native_fn_rc().is_some(),
    ))
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "null?", |args| {
        check_arity!(args, "null?", 1);
        Ok(Value::bool(match args[0].view_ref() {
            ValueViewRef::Nil => true,
            ValueViewRef::List(l) => l.is_empty(),
            _ => false,
        }))
    });

    register_fn(env, "list?", |args| {
        check_arity!(args, "list?", 1);
        Ok(Value::bool(args[0].is_list()))
    });

    register_fn(env, "vector?", |args| {
        check_arity!(args, "vector?", 1);
        Ok(Value::bool(args[0].as_vector().is_some()))
    });

    register_fn(env, "number?", |args| {
        check_arity!(args, "number?", 1);
        Ok(Value::bool(args[0].as_number().is_some()))
    });

    register_fn(env, "integer?", |args| {
        check_arity!(args, "integer?", 1);
        // Exact integers (fixnum or bignum) plus integer-valued floats (`2.0`),
        // per R7RS `integer?`.
        let is_int = args[0].is_int()
            || args[0].is_bigint()
            || matches!(args[0].as_float(), Some(f) if f.is_finite() && f.fract() == 0.0 && args[0].is_float());
        Ok(Value::bool(is_int))
    });

    register_fn(env, "float?", |args| {
        check_arity!(args, "float?", 1);
        Ok(Value::bool(args[0].is_float()))
    });

    register_fn(env, "rational?", |args| {
        check_arity!(args, "rational?", 1);
        // Exact integers and exact rationals are rational; inexact reals and
        // complex numbers are not (`exact?`/`real?` cover those distinctions).
        let ok = match args[0].as_number() {
            Some(SemaNumber::Complex(_)) | None => false,
            Some(n) => n.is_exact(),
        };
        Ok(Value::bool(ok))
    });

    register_fn(env, "exact?", |args| {
        check_arity!(args, "exact?", 1);
        Ok(Value::bool(
            args[0].as_number().is_some_and(|n| n.is_exact()),
        ))
    });

    register_fn(env, "inexact?", |args| {
        check_arity!(args, "inexact?", 1);
        Ok(Value::bool(
            args[0].as_number().is_some_and(|n| !n.is_exact()),
        ))
    });

    register_fn(env, "exact-integer?", |args| {
        check_arity!(args, "exact-integer?", 1);
        Ok(Value::bool(args[0].is_int() || args[0].is_bigint()))
    });

    register_fn(env, "complex?", |args| {
        check_arity!(args, "complex?", 1);
        // Every number is complex per R7RS (the tower's top level).
        Ok(Value::bool(args[0].as_number().is_some()))
    });

    register_fn(env, "real?", |args| {
        check_arity!(args, "real?", 1);
        Ok(Value::bool(
            args[0].as_number().is_some_and(|n| n.is_real()),
        ))
    });

    register_fn(env, "string?", |args| {
        check_arity!(args, "string?", 1);
        Ok(Value::bool(args[0].as_str().is_some()))
    });

    register_fn(env, "symbol?", |args| {
        check_arity!(args, "symbol?", 1);
        Ok(Value::bool(args[0].as_symbol_spur().is_some()))
    });

    register_fn(env, "keyword?", |args| {
        check_arity!(args, "keyword?", 1);
        Ok(Value::bool(args[0].as_keyword_spur().is_some()))
    });

    register_fn(env, "map?", |args| {
        check_arity!(args, "map?", 1);
        Ok(Value::bool(args[0].as_map_rc().is_some()))
    });

    register_fn(env, "bool?", bool_pred);
    register_fn(env, "boolean?", bool_pred);

    register_fn(env, "nil?", |args| {
        check_arity!(args, "nil?", 1);
        Ok(Value::bool(args[0].is_nil()))
    });

    register_fn(env, "fn?", procedure_pred);
    register_fn(env, "procedure?", procedure_pred);

    register_fn(env, "prompt?", |args| {
        check_arity!(args, "prompt?", 1);
        Ok(Value::bool(args[0].as_prompt_rc().is_some()))
    });

    register_fn(env, "conversation?", |args| {
        check_arity!(args, "conversation?", 1);
        Ok(Value::bool(args[0].as_conversation_rc().is_some()))
    });

    register_fn(env, "bytevector?", |args| {
        check_arity!(args, "bytevector?", 1);
        Ok(Value::bool(args[0].as_bytevector().is_some()))
    });

    register_fn(env, "record?", |args| {
        check_arity!(args, "record?", 1);
        Ok(Value::bool(args[0].as_record_rc().is_some()))
    });

    register_fn(env, "type", |args| {
        check_arity!(args, "type", 1);
        match args[0].view_ref() {
            ValueViewRef::Record(r) => Ok(Value::keyword_from_spur(r.type_tag)),
            ValueViewRef::NativeFn(nf) if nf.is_closure => Ok(Value::keyword("lambda")),
            _ => Ok(Value::keyword(args[0].type_name())),
        }
    });

    register_fn(env, "pair?", |args| {
        check_arity!(args, "pair?", 1);
        Ok(Value::bool(match args[0].view_ref() {
            ValueViewRef::List(l) => !l.is_empty(),
            _ => false,
        }))
    });

    register_fn(env, "equal?", |args| {
        check_arity!(args, "equal?", 2);
        Ok(Value::bool(args[0] == args[1]))
    });

    register_fn(env, "char?", |args| {
        check_arity!(args, "char?", 1);
        Ok(Value::bool(args[0].as_char().is_some()))
    });

    register_fn(env, "promise?", |args| {
        check_arity!(args, "promise?", 1);
        Ok(Value::bool(args[0].as_thunk_rc().is_some()))
    });

    fn promise_forced_impl(args: &[Value]) -> Result<Value, SemaError> {
        check_arity!(args, "promise-forced?", 1);
        match args[0].view_ref() {
            ValueViewRef::Thunk(t) => Ok(Value::bool(t.forced.borrow().is_some())),
            _ => Err(SemaError::type_error("promise", args[0].type_name())),
        }
    }
    register_fn(env, "promise-forced?", promise_forced_impl);
    // Canonical slash-namespaced alias (Decision #24)
    register_fn(env, "async/forced?", promise_forced_impl);

    // Silent aliases for other Lisp dialects (undocumented)
    if let Some(v) = env.get(sema_core::intern("type")) {
        env.set(sema_core::intern("type-of"), v);
    }
}
