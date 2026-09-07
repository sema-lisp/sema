use std::collections::BTreeMap;

use regex::Regex;
use sema_core::{check_arity, ArgsExt, SemaError, Value};

use crate::register_fn;

fn compile_regex(pattern: &str) -> Result<Regex, SemaError> {
    Regex::new(pattern).map_err(|e| {
        SemaError::eval(format!("invalid regex pattern: {e}")).with_hint(
            "Sema uses Rust regex syntax (no lookahead/lookbehind). See https://docs.rs/regex",
        )
    })
}

pub fn register(env: &sema_core::Env) {
    register_fn(env, "regex/match?", |args| {
        check_arity!(args, "regex/match?", 2);
        let pattern = args.str_at(0, "regex/match?")?;
        let text = args.str_at(1, "regex/match?")?;
        let re = compile_regex(pattern)?;
        Ok(Value::bool(re.is_match(text)))
    });

    register_fn(env, "regex/match", |args| {
        check_arity!(args, "regex/match", 2);
        let pattern = args.str_at(0, "regex/match")?;
        let text = args.str_at(1, "regex/match")?;
        let re = compile_regex(pattern)?;
        match re.captures(text) {
            None => Ok(Value::nil()),
            Some(caps) => {
                let full = caps.get(0).unwrap();
                let mut map = BTreeMap::new();
                map.insert(Value::keyword("match"), Value::string(full.as_str()));
                let groups: Vec<Value> = caps
                    .iter()
                    .skip(1)
                    .map(|m| match m {
                        Some(m) => Value::string(m.as_str()),
                        None => Value::nil(),
                    })
                    .collect();
                map.insert(Value::keyword("groups"), Value::list(groups));
                map.insert(Value::keyword("start"), Value::int(full.start() as i64));
                map.insert(Value::keyword("end"), Value::int(full.end() as i64));
                Ok(Value::map(map))
            }
        }
    });

    register_fn(env, "regex/find-all", |args| {
        check_arity!(args, "regex/find-all", 2);
        let pattern = args.str_at(0, "regex/find-all")?;
        let text = args.str_at(1, "regex/find-all")?;
        let re = compile_regex(pattern)?;
        let matches: Vec<Value> = re
            .find_iter(text)
            .map(|m| Value::string(m.as_str()))
            .collect();
        Ok(Value::list(matches))
    });

    register_fn(env, "regex/replace", |args| {
        check_arity!(args, "regex/replace", 3);
        let pattern = args.str_at(0, "regex/replace")?;
        let replacement = args.str_at(1, "regex/replace")?;
        let text = args.str_at(2, "regex/replace")?;
        let re = compile_regex(pattern)?;
        Ok(Value::string(&re.replace(text, replacement)))
    });

    register_fn(env, "regex/replace-all", |args| {
        check_arity!(args, "regex/replace-all", 3);
        let pattern = args.str_at(0, "regex/replace-all")?;
        let replacement = args.str_at(1, "regex/replace-all")?;
        let text = args.str_at(2, "regex/replace-all")?;
        let re = compile_regex(pattern)?;
        Ok(Value::string(&re.replace_all(text, replacement)))
    });

    register_fn(env, "regex/split", |args| {
        check_arity!(args, "regex/split", 2);
        let pattern = args.str_at(0, "regex/split")?;
        let text = args.str_at(1, "regex/split")?;
        let re = compile_regex(pattern)?;
        let parts: Vec<Value> = re.split(text).map(Value::string).collect();
        Ok(Value::list(parts))
    });
}
