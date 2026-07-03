//! Sema-on-Sema reflection: parse, format, and check Sema source from Sema.
//!
//! `read/string` / `read/all` expose the reader; `format/form` round-trips a
//! form through the formatter; `sema/check-string` / `sema/check-file` return
//! diagnostics *as data* (`{:ok bool :diagnostics [...]}`) so agents can repair
//! code in a loop instead of scraping a human-formatted error string.

use std::collections::BTreeMap;

#[cfg(not(target_arch = "wasm32"))]
use sema_core::Caps;
use sema_core::{check_arity, SemaError, Value};

use crate::register_fn;

fn kw(s: &str) -> Value {
    Value::keyword(s)
}

/// Turn a `SemaError` into a diagnostic map: `{:level :code :message :span?}`.
fn diagnostic(e: &SemaError) -> Value {
    let mut m = BTreeMap::new();
    m.insert(kw("level"), kw("error"));
    m.insert(kw("message"), Value::string(&e.to_string()));
    // Classify on the ROOT error: a reader error carrying a `.with_hint()` is
    // wrapped in SemaError::WithContext, so matching `e` directly would miss the
    // Reader arm and drop the "syntax" code + span. `inner()` unwraps the wrappers.
    let code = match e.inner() {
        SemaError::Reader { span, .. } => {
            let mut s = BTreeMap::new();
            s.insert(kw("line"), Value::int(span.line as i64));
            s.insert(kw("col"), Value::int(span.col as i64));
            s.insert(kw("end-line"), Value::int(span.end_line as i64));
            s.insert(kw("end-col"), Value::int(span.end_col as i64));
            m.insert(kw("span"), Value::map(s));
            "syntax"
        }
        SemaError::Unbound(_) => "unbound-symbol",
        SemaError::Arity { .. } => "arity",
        SemaError::Type { .. } => "type",
        _ => "error",
    };
    m.insert(kw("code"), Value::string(code));
    Value::map(m)
}

fn result_map(ok: bool, diagnostics: Vec<Value>) -> Value {
    let mut m = BTreeMap::new();
    m.insert(kw("ok"), Value::bool(ok));
    m.insert(kw("diagnostics"), Value::list(diagnostics));
    Value::map(m)
}

/// Parse, then compile, collecting the first error as a diagnostic.
fn check_source(src: &str) -> Value {
    match sema_reader::read_many(src) {
        Err(e) => result_map(false, vec![diagnostic(&e)]),
        Ok(forms) => match sema_vm::compile_program(&forms, None) {
            Err(e) => result_map(false, vec![diagnostic(&e)]),
            Ok(_) => result_map(true, vec![]),
        },
    }
}

#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
pub fn register(env: &sema_core::Env, sandbox: &sema_core::Sandbox) {
    // read/string — parse exactly one form (canonical namespaced name; the bare
    // `read` builtin is the legacy alias).
    register_fn(env, "read/string", |args| {
        check_arity!(args, "read/string", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        sema_reader::read(s)
    });

    // read/all — parse every top-level form into a list.
    register_fn(env, "read/all", |args| {
        check_arity!(args, "read/all", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(Value::list(sema_reader::read_many(s)?))
    });

    // format/form — pretty-print a form (the formatter's output for the form's
    // re-readable source). Falls back to the unformatted source if the form
    // can't be formatted (e.g. a non-code value).
    register_fn(env, "format/form", |args| {
        check_arity!(args, "format/form", 1);
        let src = format!("{}", args[0]);
        let out = sema_fmt::format_source(&src, &sema_fmt::FormatOptions::default()).unwrap_or(src);
        Ok(Value::string(out.trim_end()))
    });

    // sema/check-string — diagnostics for a source string, as data.
    register_fn(env, "sema/check-string", |args| {
        check_arity!(args, "sema/check-string", 1);
        let s = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        Ok(check_source(s))
    });

    // sema/check-file — like check-string but reads a file first.
    // Touches the real filesystem (not the VFS), so it's native-only.
    #[cfg(not(target_arch = "wasm32"))]
    crate::register_fn_gated(env, sandbox, Caps::FS_READ, "sema/check-file", |args| {
        check_arity!(args, "sema/check-file", 1);
        let path = args[0]
            .as_str()
            .ok_or_else(|| SemaError::type_error("string", args[0].type_name()))?;
        match std::fs::read_to_string(path) {
            Ok(src) => Ok(check_source(&src)),
            Err(e) => {
                let mut m = BTreeMap::new();
                m.insert(kw("level"), kw("error"));
                m.insert(kw("code"), Value::string("io"));
                m.insert(kw("message"), Value::string(&format!("{path}: {e}")));
                Ok(result_map(false, vec![Value::map(m)]))
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sema_core::{EvalContext, Sandbox};

    fn env() -> sema_core::Env {
        let e = sema_core::Env::new();
        register(&e, &Sandbox::allow_all());
        e
    }

    fn call(env: &sema_core::Env, name: &str, args: &[Value]) -> Value {
        let f = env.get_str(name).expect("fn registered");
        let nf = f.as_native_fn_ref().expect("native fn");
        (nf.func)(&EvalContext::default(), args).expect("call ok")
    }

    #[test]
    fn read_all_parses_forms() {
        let e = env();
        let v = call(&e, "read/all", &[Value::string("(+ 1 2) (* 3 4)")]);
        assert_eq!(v.as_list().map(|l| l.len()), Some(2));
    }

    #[test]
    fn check_string_ok_and_error() {
        let e = env();
        let ok = call(&e, "sema/check-string", &[Value::string("(+ 1 2)")]);
        assert_eq!(
            ok.as_map_ref().unwrap().get(&kw("ok")),
            Some(&Value::bool(true))
        );

        let bad = call(&e, "sema/check-string", &[Value::string("(+ 1 2")]); // unbalanced
        let m = bad.as_map_ref().unwrap();
        assert_eq!(m.get(&kw("ok")), Some(&Value::bool(false)));
        let diags = m.get(&kw("diagnostics")).and_then(|d| d.as_list()).unwrap();
        assert_eq!(diags.len(), 1);
    }

    #[test]
    fn format_form_roundtrips() {
        let e = env();
        // (read/string "(+  1   2)") → a form → format/form → tidy source.
        let form = call(&e, "read/string", &[Value::string("(+  1   2)")]);
        let pretty = call(&e, "format/form", &[form]);
        assert_eq!(pretty.as_str(), Some("(+ 1 2)"));
    }
}
