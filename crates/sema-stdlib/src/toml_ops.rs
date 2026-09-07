use std::collections::BTreeMap;

use sema_core::{check_arity, ArgsExt, SemaError, Value, ValueView};

use crate::register_fn;

pub fn register(env: &sema_core::Env) {
    register_fn(env, "toml/decode", |args| {
        check_arity!(args, "toml/decode", 1);
        let s = args.str_at(0, "toml/decode")?;
        let table: toml::Table = s.parse().map_err(|e| {
            SemaError::eval(format!("toml/decode: parse error: {e}")).with_hint(
                "toml/decode expects a TOML document (keys use = not :, strings use double quotes)",
            )
        })?;
        Ok(toml_table_to_value(&table))
    });

    register_fn(env, "toml/encode", |args| {
        check_arity!(args, "toml/encode", 1);
        let toml_val = value_to_toml(&args[0])?;
        match toml_val {
            toml::Value::Table(t) => {
                let s = toml::to_string(&t)
                    .map_err(|e| SemaError::eval(format!("toml/encode: {e}")))?;
                Ok(Value::string(&s))
            }
            _ => Err(SemaError::eval(
                "toml/encode: top-level value must be a map",
            )),
        }
    });
}

fn toml_to_value(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::string(s),
        toml::Value::Integer(n) => Value::int(*n),
        toml::Value::Float(f) => Value::float(*f),
        toml::Value::Boolean(b) => Value::bool(*b),
        toml::Value::Datetime(dt) => Value::string(&dt.to_string()),
        toml::Value::Array(arr) => Value::list(arr.iter().map(toml_to_value).collect()),
        toml::Value::Table(t) => toml_table_to_value(t),
    }
}

fn toml_table_to_value(table: &toml::map::Map<String, toml::Value>) -> Value {
    let mut map = BTreeMap::new();
    for (k, v) in table {
        map.insert(Value::keyword(k), toml_to_value(v));
    }
    Value::map(map)
}

fn value_to_toml(val: &Value) -> Result<toml::Value, SemaError> {
    match val.view() {
        ValueView::Nil => Err(SemaError::eval("toml/encode: cannot encode nil")),
        ValueView::Bool(b) => Ok(toml::Value::Boolean(b)),
        ValueView::Int(n) => Ok(toml::Value::Integer(n)),
        ValueView::Float(f) => Ok(toml::Value::Float(f)),
        ValueView::String(s) => Ok(toml::Value::String(s.to_string())),
        ValueView::Keyword(s) => Ok(toml::Value::String(sema_core::resolve(s))),
        ValueView::Symbol(s) => Ok(toml::Value::String(sema_core::resolve(s))),
        ValueView::List(items) | ValueView::Vector(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(value_to_toml).collect();
            Ok(toml::Value::Array(arr?))
        }
        ValueView::Map(map) => {
            let mut t = toml::map::Map::new();
            for (k, v) in map.iter() {
                t.insert(sema_core::key_to_string(k), value_to_toml(v)?);
            }
            Ok(toml::Value::Table(t))
        }
        ValueView::HashMap(map) => {
            let mut t = toml::map::Map::new();
            for (k, v) in map.iter() {
                t.insert(sema_core::key_to_string(k), value_to_toml(v)?);
            }
            Ok(toml::Value::Table(t))
        }
        _ => Err(SemaError::eval(format!(
            "toml/encode: cannot encode {}",
            val.type_name()
        ))),
    }
}
