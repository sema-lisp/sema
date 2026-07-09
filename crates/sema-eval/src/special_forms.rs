use std::rc::Rc;

use sema_core::{
    intern, resolve, Agent, Env, EvalContext, Record, SemaError, Spur, ToolDefinition, Value,
};

use crate::eval::{self, Trampoline};

/// Canonical list of all special form names recognized by the evaluator.
///
/// This is the single source of truth — used by the REPL for completion,
/// the LSP for highlighting, and anywhere else that needs to enumerate special forms.
pub const SPECIAL_FORM_NAMES: &[&str] = &[
    // Core language
    "and",
    "async",
    "await",
    "begin",
    "case",
    "cond",
    "define",
    "define-record-type",
    "define-values",
    "defmacro",
    "defmethod",
    "defmulti",
    "defun",
    "delay",
    "do",
    "eval",
    "fn",
    "force",
    "if",
    "lambda",
    "let",
    "let*",
    "let*-values",
    "let-values",
    "letrec",
    "macroexpand",
    "match",
    "or",
    "quasiquote",
    "quote",
    "set!",
    "throw",
    "try",
    "unless",
    "when",
    "while",
    // Modules
    "export",
    "import",
    "load",
    "module",
    // LLM primitives
    "defagent",
    "deftool",
    "message",
    "prompt",
    // Silent aliases for other Lisp dialects (undocumented)
    "def",
    "defn",
    "progn",
];

/// Build a `ToolDefinition` from already-evaluated values and bind it in `env`.
/// The VM's `__vm-deftool` native passes the pre-evaluated description /
/// parameters / handler straight here.
pub(crate) fn register_tool(
    name: &str,
    description: Value,
    parameters: Value,
    handler: Value,
    env: &Env,
) -> Result<Value, SemaError> {
    let description = description
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", description.type_name()))?
        .to_string();
    let tool = Value::tool_def(ToolDefinition {
        name: name.to_string(),
        description,
        parameters,
        handler,
    });
    env.set(intern(name), tool.clone());
    Ok(tool)
}

/// Build an `Agent` from an already-evaluated options map and bind it in `env`.
/// The VM's `__vm-defagent` native passes the pre-evaluated options map here.
pub(crate) fn register_agent(name: &str, opts: Value, env: &Env) -> Result<Value, SemaError> {
    let opts_map = opts
        .as_map_rc()
        .ok_or_else(|| SemaError::type_error("map", opts.type_name()))?;

    let system = opts_map
        .get(&Value::keyword("system"))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let tools = opts_map
        .get(&Value::keyword("tools"))
        .map(|v| {
            if let Some(l) = v.as_list() {
                l.to_vec()
            } else if let Some(v) = v.as_vector() {
                v.to_vec()
            } else {
                vec![]
            }
        })
        .unwrap_or_default();

    let max_turns = opts_map
        .get(&Value::keyword("max-turns"))
        .and_then(|v| v.as_int())
        .unwrap_or(10) as usize;

    let model = opts_map
        .get(&Value::keyword("model"))
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let agent = Value::agent(Agent {
        name: name.to_string(),
        system,
        tools,
        max_turns,
        model,
    });
    env.set(intern(name), agent.clone());
    Ok(agent)
}

fn is_module_entry_spec(spec: &str) -> bool {
    sema_core::resolve::is_package_import(spec)
        || (!spec.ends_with(".sema")
            && !spec.starts_with("./")
            && !spec.starts_with("../")
            && !spec.starts_with('/'))
}

fn module_file_path(
    spec: &str,
    resolved_path: &std::path::Path,
    direct_hit: bool,
) -> std::path::PathBuf {
    if direct_hit && is_module_entry_spec(spec) {
        resolved_path.join("__entry__")
    } else {
        resolved_path.to_path_buf()
    }
}

fn resolve_embedded_file(
    ctx: &EvalContext,
    spec: &str,
) -> Option<(std::path::PathBuf, std::path::PathBuf, Vec<u8>)> {
    // Archive keys are clean, lexically-normalized, root-relative paths (e.g.
    // "util.sema", "lib/util.sema"). Look the spec up in the same normalized form
    // — resolving "./", "../", and interior "." — so every spelling that names
    // the same module hits the one key. Try the spec as a root-relative key
    // first, then relative to the importing file's directory (for nested
    // imports). Normalization also becomes the cache/current_file identity, so a
    // module resolved via different spellings is only evaluated once.
    if let Some(norm) = sema_core::vfs::normalize_path(std::path::Path::new(spec)) {
        let key = std::path::PathBuf::from(&norm);
        if let Some(bytes) = ctx.get_embedded_file(&key) {
            let file_path = module_file_path(spec, &key, true);
            return Some((key, file_path, bytes));
        }
    }

    let base_dir = ctx.current_file_dir()?;
    if let Some(norm) = sema_core::vfs::normalize_path(&base_dir.join(spec)) {
        let key = std::path::PathBuf::from(&norm);
        if let Some(bytes) = ctx.get_embedded_file(&key) {
            let file_path = module_file_path(spec, &key, false);
            return Some((key, file_path, bytes));
        }
    }
    None
}

fn eval_bytes_in_env(
    op_name: &str,
    path_str: &str,
    exec_path: &std::path::Path,
    bytes: &[u8],
    env: &Env,
    ctx: &EvalContext,
) -> Result<Value, SemaError> {
    ctx.push_file_path(exec_path.to_path_buf());
    let eval_result = (|| {
        if sema_vm::is_bytecode_file(bytes) {
            let result = sema_vm::deserialize_from_bytes(bytes)?;
            return eval::execute_compile_result(ctx, Rc::new(env.clone()), result);
        }

        let content = String::from_utf8(bytes.to_vec())
            .map_err(|e| SemaError::Io(format!("{op_name} {path_str}: invalid UTF-8: {e}")))?;
        let (exprs, spans) = sema_reader::read_many_with_spans(&content)?;
        ctx.merge_span_table(spans.clone());

        eval::eval_module_body_vm(ctx, env, &exprs, &spans, Some(exec_path.to_path_buf()))
    })();
    ctx.pop_file_path();
    eval_result
}

fn import_module_from_bytes(
    path_str: &str,
    resolved_path: std::path::PathBuf,
    file_path: std::path::PathBuf,
    content_bytes: Vec<u8>,
    selective: &[String],
    env: &Env,
    ctx: &EvalContext,
) -> Result<Trampoline, SemaError> {
    if let Some(cached) = ctx.get_cached_module(&resolved_path) {
        copy_exports_to_env(&cached, selective, env)?;
        return Ok(Trampoline::Value(Value::nil()));
    }

    // Scoped so the load-stack guard drops right after evaluation (before we
    // cache), matching the original end-of-load point.
    let load_result: Result<std::collections::BTreeMap<String, Value>, SemaError> = {
        let _guard = ctx.enter_module_load(resolved_path.clone())?;
        let module_env = eval::create_module_env(env);
        ctx.push_file_path(file_path.clone());
        ctx.clear_module_exports();

        let eval_result = (|| {
            if sema_vm::is_bytecode_file(&content_bytes) {
                let result = sema_vm::deserialize_from_bytes(&content_bytes)?;
                eval::execute_compile_result(ctx, Rc::new(module_env.clone()), result)?;
            } else {
                let content = String::from_utf8(content_bytes).map_err(|e| {
                    SemaError::Io(format!("import {path_str}: invalid UTF-8 in module: {e}"))
                })?;
                let (exprs, spans) = sema_reader::read_many_with_spans(&content)?;
                ctx.merge_span_table(spans.clone());
                eval::eval_module_body_vm(
                    ctx,
                    &module_env,
                    &exprs,
                    &spans,
                    Some(file_path.clone()),
                )?;
            }
            Ok(())
        })();

        ctx.pop_file_path();
        let declared = ctx.take_module_exports();
        eval_result.map(|()| collect_module_exports(&module_env, declared.as_deref()))
    };
    let exports = load_result?;

    ctx.cache_module(resolved_path, exports.clone());
    copy_exports_to_env(&exports, selective, env)?;
    Ok(Trampoline::Value(Value::nil()))
}

/// (import "path.sema") or (import "path.sema" sym1 sym2)
pub(crate) fn eval_import(
    args: &[Value],
    env: &Env,
    ctx: &EvalContext,
) -> Result<Trampoline, SemaError> {
    if args.is_empty() {
        return Err(SemaError::arity("import", "1+", 0));
    }
    // Gate filesystem/VFS access behind the sandbox (EVAL-3). Without this, a
    // restricted sandbox could be bypassed by importing a module.
    ctx.sandbox.check(sema_core::Caps::FS_READ, "import")?;
    let path_val = args[0].clone(); // already evaluated by the VM (`__vm-import`/`__vm-load`)
    let path_str = path_val
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", path_val.type_name()))?;

    // Imported modules run on a separate VM (per-form) and bypass the debug
    // loop, so breakpoints in them never hit. Warn once per debug session
    // (no-op outside a session). See §7.4 #4.
    crate::debug_session::warn_load_bypass_once("import", path_str);

    // Selective import names
    let selective: Vec<String> = args[1..]
        .iter()
        .map(|v| {
            v.as_symbol()
                .map(|s| s.to_string())
                .ok_or_else(|| SemaError::eval("import: selective names must be symbols"))
        })
        .collect::<Result<_, _>>()?;

    if let Some((resolved_path, file_path, content_bytes)) = resolve_embedded_file(ctx, path_str) {
        return import_module_from_bytes(
            path_str,
            resolved_path,
            file_path,
            content_bytes,
            &selective,
            env,
            ctx,
        );
    }

    // Check VFS first — bundled executables have packages embedded in the VFS
    // and won't have them installed on the filesystem.
    if sema_core::vfs::is_vfs_active() {
        let base_dir = ctx
            .current_file_dir()
            .map(|d| d.to_string_lossy().to_string());

        // The canonical, normalized key that actually matched (resolving
        // "./"/".."). Using it as the cache identity + current_file means every
        // spelling of a module (e.g. "shared.sema" vs "sub/../shared.sema")
        // dedups to one evaluation and roots its own imports correctly.
        if let Some(key) = sema_core::vfs::vfs_resolve_key(path_str, base_dir.as_deref()) {
            let resolved_vfs_path = std::path::PathBuf::from(&key);

            // Package/dir entries have no ".sema" filename component; append a
            // synthetic one so current_file_dir() returns the package directory.
            let is_package = sema_core::resolve::is_package_import(&key) || !key.ends_with(".sema");
            let file_path = if is_package {
                resolved_vfs_path.join("__entry__")
            } else {
                resolved_vfs_path.clone()
            };

            if let Some(content_bytes) = sema_core::vfs::vfs_read(&key) {
                return import_module_from_bytes(
                    path_str,
                    resolved_vfs_path,
                    file_path,
                    content_bytes,
                    &selective,
                    env,
                    ctx,
                );
            }
        }
    }

    // Resolve path: package imports first, then relative/absolute
    let resolved = if sema_core::resolve::is_package_import(path_str) {
        sema_core::resolve::resolve_package_import(path_str)?
    } else if std::path::Path::new(path_str).is_absolute() {
        std::path::PathBuf::from(path_str)
    } else if let Some(dir) = ctx.current_file_dir() {
        dir.join(path_str)
    } else {
        std::path::PathBuf::from(path_str)
    };

    // Check cache for preloaded modules (before canonicalize, which requires a real file).
    if let Some(cached) = ctx.get_cached_module(&resolved) {
        copy_exports_to_env(&cached, &selective, env)?;
        return Ok(Trampoline::Value(Value::nil()));
    }

    let canonical = resolved
        .canonicalize()
        .map_err(|e| SemaError::Io(format!("import {path_str}: {e}")))?;

    // Check cache for on-disk modules
    if let Some(cached) = ctx.get_cached_module(&canonical) {
        copy_exports_to_env(&cached, &selective, env)?;
        return Ok(Trampoline::Value(Value::nil()));
    }

    let content_bytes =
        std::fs::read(&canonical).map_err(|e| SemaError::Io(format!("import {path_str}: {e}")))?;
    import_module_from_bytes(
        path_str,
        canonical.clone(),
        canonical,
        content_bytes,
        &selective,
        env,
        ctx,
    )
}

/// Collect exported bindings from a module env
fn collect_module_exports(
    module_env: &Env,
    declared: Option<&[String]>,
) -> std::collections::BTreeMap<String, Value> {
    match declared {
        Some(names) => {
            let mut exports = std::collections::BTreeMap::new();
            for name in names {
                let spur = intern(name);
                if let Some(val) = module_env.get_local(spur) {
                    exports.insert(name.clone(), val);
                }
            }
            exports
        }
        None => {
            let mut exports = std::collections::BTreeMap::new();
            module_env.iter_bindings(|spur, val| {
                exports.insert(resolve(spur), val.clone());
            });
            exports
        }
    }
}

/// Copy exports into the caller environment
fn copy_exports_to_env(
    exports: &std::collections::BTreeMap<String, Value>,
    selective: &[String],
    env: &Env,
) -> Result<(), SemaError> {
    if selective.is_empty() {
        for (name, val) in exports {
            env.set(intern(name), val.clone());
        }
    } else {
        for name in selective {
            let val = exports.get(name).ok_or_else(|| {
                SemaError::eval(format!("import: module does not export '{name}'"))
            })?;
            env.set(intern(name), val.clone());
        }
    }
    Ok(())
}

/// (load "file.sema") — read and evaluate a file in the current environment
pub(crate) fn eval_load(
    args: &[Value],
    env: &Env,
    ctx: &EvalContext,
) -> Result<Trampoline, SemaError> {
    if args.len() != 1 {
        return Err(SemaError::arity("load", "1", args.len()));
    }
    ctx.sandbox.check(sema_core::Caps::FS_READ, "load")?;
    let path_val = args[0].clone(); // already evaluated by the VM (`__vm-import`/`__vm-load`)
    let path_str = path_val
        .as_str()
        .ok_or_else(|| SemaError::type_error("string", path_val.type_name()))?;

    // Loaded code runs on a separate VM (per-form) and bypasses the debug loop, so
    // breakpoints in it never hit. Warn once per debug session (no-op outside a
    // session). See §7.4 #4.
    crate::debug_session::warn_load_bypass_once("load", path_str);

    // Resolve path relative to current file
    let resolved = if std::path::Path::new(path_str).is_absolute() {
        std::path::PathBuf::from(path_str)
    } else if let Some(dir) = ctx.current_file_dir() {
        dir.join(path_str)
    } else {
        std::path::PathBuf::from(path_str)
    };

    // `enter_module_load` guards against cycles (a loads b loads a…), which would
    // otherwise recurse until the stack overflows. Keyed on the resolved
    // identity, so a completed load can still be re-loaded — only an in-progress
    // cycle errors. The guard pops the stack on any exit path.
    if let Some((resolved_key, file_path, content_bytes)) = resolve_embedded_file(ctx, path_str) {
        let _guard = ctx.enter_module_load(resolved_key)?;
        let result = eval_bytes_in_env("load", path_str, &file_path, &content_bytes, env, ctx)?;
        return Ok(Trampoline::Value(result));
    }

    // Check VFS before hitting the filesystem
    if sema_core::vfs::is_vfs_active() {
        let base_dir = ctx
            .current_file_dir()
            .map(|d| d.to_string_lossy().to_string());
        if let Some(key) = sema_core::vfs::vfs_resolve_key(path_str, base_dir.as_deref()) {
            if let Some(content_bytes) = sema_core::vfs::vfs_read(&key) {
                let vfs_path = std::path::PathBuf::from(&key);
                let _guard = ctx.enter_module_load(vfs_path.clone())?;
                let result =
                    eval_bytes_in_env("load", path_str, &vfs_path, &content_bytes, env, ctx)?;
                return Ok(Trampoline::Value(result));
            }
        }
    }

    let canonical = resolved
        .canonicalize()
        .map_err(|e| SemaError::Io(format!("load {}: {e}", resolved.display())))?;
    let content_bytes = std::fs::read(&canonical)
        .map_err(|e| SemaError::Io(format!("load {}: {e}", canonical.display())))?;
    let _guard = ctx.enter_module_load(canonical.clone())?;
    let result = eval_bytes_in_env("load", path_str, &canonical, &content_bytes, env, ctx)?;
    Ok(Trampoline::Value(result))
}

/// (define-record-type <name> (<ctor> <field> ...) <pred> (<field> <accessor> [<mutator>]) ...)
pub(crate) fn eval_define_record_type(args: &[Value], env: &Env) -> Result<Trampoline, SemaError> {
    if args.len() < 3 {
        return Err(SemaError::eval(
            "define-record-type: requires at least type name, constructor, and predicate",
        ));
    }

    let type_name = args[0]
        .as_symbol()
        .ok_or_else(|| SemaError::eval("define-record-type: type name must be a symbol"))?;
    let type_tag = intern(&type_name);

    let ctor_spec = args[1]
        .as_list()
        .ok_or_else(|| SemaError::eval("define-record-type: constructor spec must be a list"))?;
    if ctor_spec.is_empty() {
        return Err(SemaError::eval(
            "define-record-type: constructor spec must have a name",
        ));
    }
    let ctor_name = ctor_spec[0]
        .as_symbol()
        .ok_or_else(|| SemaError::eval("define-record-type: constructor name must be a symbol"))?;
    let field_names: Vec<String> = ctor_spec[1..]
        .iter()
        .map(|v| {
            v.as_symbol()
                .ok_or_else(|| SemaError::eval("define-record-type: field name must be a symbol"))
        })
        .collect::<Result<_, _>>()?;
    let field_name_spurs: Vec<Spur> = field_names.iter().map(|name| intern(name)).collect();
    let field_count = field_names.len();

    let pred_name = args[2]
        .as_symbol()
        .ok_or_else(|| SemaError::eval("define-record-type: predicate must be a symbol"))?;

    let ctor_name_clone = ctor_name.clone();
    let record_field_names = field_name_spurs.clone();
    env.set_str(
        &ctor_name,
        Value::native_fn(sema_core::NativeFn::simple(
            ctor_name.clone(),
            move |args: &[Value]| {
                if args.len() != field_count {
                    return Err(SemaError::arity(
                        &ctor_name_clone,
                        field_count.to_string(),
                        args.len(),
                    ));
                }
                Ok(Value::record(Record {
                    type_tag,
                    field_names: record_field_names.clone(),
                    fields: args.to_vec(),
                }))
            },
        )),
    );

    let pred_name_for_closure = pred_name.clone();
    let pred_name_for_set = pred_name.clone();
    env.set_str(
        &pred_name_for_set,
        Value::native_fn(sema_core::NativeFn::simple(
            pred_name,
            move |args: &[Value]| {
                if args.len() != 1 {
                    return Err(SemaError::arity(&pred_name_for_closure, "1", args.len()));
                }
                Ok(Value::bool(
                    args[0].as_record().is_some_and(|r| r.type_tag == type_tag),
                ))
            },
        )),
    );

    for field_spec_val in &args[3..] {
        let field_spec = field_spec_val
            .as_list()
            .ok_or_else(|| SemaError::eval("define-record-type: field spec must be a list"))?;
        if field_spec.len() < 2 {
            return Err(SemaError::eval(
                "define-record-type: field spec must have at least (field-name accessor)",
            ));
        }

        let field_name = field_spec[0]
            .as_symbol()
            .ok_or_else(|| SemaError::eval("define-record-type: field name must be a symbol"))?;

        let field_idx = field_names
            .iter()
            .position(|n| n == &field_name)
            .ok_or_else(|| {
                SemaError::eval(format!(
                    "define-record-type: field '{field_name}' not in constructor"
                ))
            })?;

        let accessor_name = field_spec[1]
            .as_symbol()
            .ok_or_else(|| SemaError::eval("define-record-type: accessor must be a symbol"))?;

        let accessor_name_for_closure = accessor_name.clone();
        let accessor_name_for_set = accessor_name.clone();
        let type_name_for_err = type_name.clone();
        env.set_str(
            &accessor_name_for_set,
            Value::native_fn(sema_core::NativeFn::simple(
                accessor_name,
                move |args: &[Value]| {
                    if args.len() != 1 {
                        return Err(SemaError::arity(
                            &accessor_name_for_closure,
                            "1",
                            args.len(),
                        ));
                    }
                    match args[0].as_record() {
                        Some(r) if r.type_tag == type_tag => Ok(r.fields[field_idx].clone()),
                        _ => Err(SemaError::type_error(
                            &type_name_for_err,
                            args[0].type_name(),
                        )),
                    }
                },
            )),
        );
    }

    Ok(Trampoline::Value(Value::nil()))
}

/// Parse parameter list, handling rest params (e.g., `(a b . rest)`)
pub(crate) fn parse_params(names: &[Spur]) -> (Vec<Spur>, Option<Spur>) {
    let dot = intern(".");
    if let Some(pos) = names.iter().position(|s| *s == dot) {
        let params = names[..pos].to_vec();
        let rest = if pos + 1 < names.len() {
            Some(names[pos + 1])
        } else {
            None
        };
        (params, rest)
    } else {
        (names.to_vec(), None)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::eval::Interpreter;

    fn compile_source(interp: &Interpreter, source: &str) -> Vec<u8> {
        let result = interp.compile_to_bytecode(source).unwrap();
        sema_vm::serialize_to_bytes(&result, 0).unwrap()
    }

    #[test]
    fn import_reads_embedded_bytecode_modules() {
        let interp = Interpreter::new();
        let module = r#"
            (module util (export double)
              (define (double x) (* x 2)))
        "#;
        let bytes = compile_source(&interp, module);
        interp
            .ctx
            .set_embedded_file(PathBuf::from("lib/util.sema"), bytes);

        let result = interp
            .eval_str(r#"(import "lib/util.sema") (double 21)"#)
            .unwrap();
        assert_eq!(result, Value::int(42));
    }

    /// Set an embedded module (source form) under `key`.
    fn embed(interp: &Interpreter, key: &str, source: &str) {
        let bytes = compile_source(interp, source);
        interp.ctx.set_embedded_file(PathBuf::from(key), bytes);
    }

    // Embedded imports must resolve regardless of how the spec is spelled — a
    // leading "./", subdirectories, nested relatives, and "../" parents all name
    // the same clean archive key. (These broke before the normalization fix.)

    #[test]
    fn embedded_import_dotslash_prefix() {
        let interp = Interpreter::new();
        embed(&interp, "u.sema", "(module u (export v) (define v 7))");
        assert_eq!(
            interp.eval_str(r#"(import "./u.sema") v"#).unwrap(),
            Value::int(7)
        );
    }

    #[test]
    fn embedded_import_subdir_dotslash() {
        let interp = Interpreter::new();
        embed(&interp, "lib/u.sema", "(module u (export v) (define v 8))");
        assert_eq!(
            interp.eval_str(r#"(import "./lib/u.sema") v"#).unwrap(),
            Value::int(8)
        );
    }

    #[test]
    fn embedded_import_nested_relative() {
        // entry → lib/a, a (in lib/) → ./b resolves to lib/b via current_file.
        let interp = Interpreter::new();
        embed(
            &interp,
            "lib/b.sema",
            "(module b (export bv) (define bv 5))",
        );
        embed(
            &interp,
            "lib/a.sema",
            r#"(module a (export av) (import "./b.sema") (define av bv))"#,
        );
        assert_eq!(
            interp.eval_str(r#"(import "./lib/a.sema") av"#).unwrap(),
            Value::int(5)
        );
    }

    #[test]
    fn embedded_import_parent_relative() {
        // a in sub/ imports ../shared → normalizes to the root key "shared.sema".
        let interp = Interpreter::new();
        embed(
            &interp,
            "shared.sema",
            "(module s (export sv) (define sv 9))",
        );
        embed(
            &interp,
            "sub/a.sema",
            r#"(module a (export av) (import "../shared.sema") (define av sv))"#,
        );
        assert_eq!(
            interp.eval_str(r#"(import "./sub/a.sema") av"#).unwrap(),
            Value::int(9)
        );
    }

    #[test]
    fn embedded_circular_import_errors_gracefully() {
        let interp = Interpreter::new();
        embed(
            &interp,
            "a.sema",
            r#"(module a (export av) (import "./b.sema") (define av 1))"#,
        );
        embed(
            &interp,
            "b.sema",
            r#"(module b (export bv) (import "./a.sema") (define bv 2))"#,
        );
        let err = interp.eval_str(r#"(import "./a.sema")"#).unwrap_err();
        assert!(
            err.to_string().contains("cyclic"),
            "expected a cyclic-import error, got: {err}"
        );
    }

    #[test]
    fn embedded_circular_load_errors_instead_of_overflowing() {
        // Before the fix this recursed until the stack overflowed (process abort).
        let interp = Interpreter::new();
        embed(&interp, "a.sema", r#"(load "./b.sema")"#);
        embed(&interp, "b.sema", r#"(load "./a.sema")"#);
        let err = interp.eval_str(r#"(load "./a.sema")"#).unwrap_err();
        assert!(
            err.to_string().contains("cyclic"),
            "expected a cyclic error, got: {err}"
        );
    }

    #[test]
    fn embedded_import_redundant_path_components() {
        // Absurd but valid spelling that normalizes to the clean key "lib/u.sema".
        let interp = Interpreter::new();
        embed(&interp, "lib/u.sema", "(module u (export v) (define v 3))");
        assert_eq!(
            interp
                .eval_str(r#"(import "./lib/./.././lib/u.sema") v"#)
                .unwrap(),
            Value::int(3)
        );
    }

    #[test]
    fn embedded_import_deep_weird_chain() {
        // Every hop uses a weird spelling and climbs subdirs then back to root.
        let interp = Interpreter::new();
        embed(
            &interp,
            "top.sema",
            r#"(module top (export MARK) (define MARK 42))"#,
        );
        embed(
            &interp,
            "a/b/c/m3.sema",
            r#"(module m3 (export MARK) (import "../../../top.sema"))"#,
        );
        embed(
            &interp,
            "a/b/m2.sema",
            r#"(module m2 (export MARK) (import "./c/../c/m3.sema"))"#,
        );
        embed(
            &interp,
            "a/m1.sema",
            r#"(module m1 (export MARK) (import "../a/./b/m2.sema"))"#,
        );
        assert_eq!(
            interp.eval_str(r#"(import "././a/m1.sema") MARK"#).unwrap(),
            Value::int(42)
        );
    }

    #[test]
    fn embedded_import_unicode_and_emoji_paths() {
        // UTF-8 module names + directories resolve byte-for-byte.
        let interp = Interpreter::new();
        embed(
            &interp,
            "lïb-café/µtil.sema",
            "(module u (export v) (define v 1))",
        );
        embed(
            &interp,
            "rocket-🚀.sema",
            "(module r (export w) (define w 2))",
        );
        assert_eq!(
            interp
                .eval_str(r#"(import "./lïb-café/µtil.sema") v"#)
                .unwrap(),
            Value::int(1)
        );
        assert_eq!(
            interp.eval_str(r#"(import "./rocket-🚀.sema") w"#).unwrap(),
            Value::int(2)
        );
    }

    #[test]
    fn embedded_import_escaping_root_is_rejected() {
        // A spec that lexically escapes the archive root must NOT resolve from the
        // embedded store (it would otherwise fall through to the host fs).
        let interp = Interpreter::new();
        embed(&interp, "u.sema", "(module u (export v) (define v 9))");
        // "../u.sema" pops above the root → no embedded hit → no such file.
        assert!(interp.eval_str(r#"(import "../u.sema") v"#).is_err());
    }

    #[test]
    fn embedded_import_combining_mark_byte_identical() {
        // A decomposed (NFD) combining-mark module name resolves when the import
        // string uses the identical byte sequence ("cafe" + U+0301).
        let interp = Interpreter::new();
        embed(
            &interp,
            "cafe\u{0301}.sema",
            "(module m (export v) (define v 5))",
        );
        assert_eq!(
            interp
                .eval_str("(import \"./cafe\u{0301}.sema\") v")
                .unwrap(),
            Value::int(5)
        );
    }

    #[test]
    fn embedded_resolution_is_byte_exact_for_unicode() {
        // Embedded/.vfs keys match byte-for-byte with no Unicode NFC/NFD folding,
        // so a precomposed (NFC "café" = U+00E9) import does NOT resolve a
        // decomposed (NFD "cafe" + U+0301) key. (macOS's fs is
        // normalization-insensitive and would resolve it natively; the archive is
        // byte-exact + portable, so keep the filename and import spec in the same
        // normalization form.) This pins that documented behavior.
        let interp = Interpreter::new();
        embed(
            &interp,
            "cafe\u{0301}.sema",
            "(module m (export v) (define v 5))",
        );
        assert!(interp
            .eval_str("(import \"./caf\u{00e9}.sema\") v")
            .is_err());
    }

    #[test]
    fn compiled_entry_imports_embedded_package_modules() {
        let interp = Interpreter::new();
        let module = r#"
            (module util (export answer)
              (define answer 42))
        "#;
        let bytes = compile_source(&interp, module);
        interp
            .ctx
            .set_embedded_file(PathBuf::from("json-utils"), bytes);

        let result = interp
            .eval_str_compiled(r#"(import "json-utils") answer"#)
            .unwrap();
        assert_eq!(result, Value::int(42));
    }

    #[test]
    fn load_executes_embedded_bytecode_files() {
        let interp = Interpreter::new();
        let file = r#"
            (define loaded-value 7)
            (set! loaded-value (+ loaded-value 5))
        "#;
        let bytes = compile_source(&interp, file);
        interp
            .ctx
            .set_embedded_file(PathBuf::from("defs.sema"), bytes);

        let result = interp
            .eval_str(r#"(load "defs.sema") loaded-value"#)
            .unwrap();
        assert_eq!(result, Value::int(12));
    }

    #[test]
    fn load_bytecode_preserves_relative_resolution() {
        let interp = Interpreter::new();
        interp.ctx.set_embedded_file(
            PathBuf::from("nested/helper.sema"),
            br#"(define helper-value 41)"#.to_vec(),
        );
        let file = r#"
            (load "helper.sema")
            (define loaded-value (+ helper-value 1))
        "#;
        let bytes = compile_source(&interp, file);
        interp
            .ctx
            .set_embedded_file(PathBuf::from("nested/main.sema"), bytes);

        let result = interp
            .eval_str(r#"(load "nested/main.sema") loaded-value"#)
            .unwrap();
        assert_eq!(result, Value::int(42));
    }
}
