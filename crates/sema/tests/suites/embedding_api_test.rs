//! Tests for the public embedding API (`sema::Interpreter`), which wraps
//! `sema_eval::Interpreter`. Regression guard for M6: the builder must register
//! the VM delegates + prelude so embedders get import/load and prelude macros on
//! the VM (the sole evaluator), and `preload_module` must run on the VM.

use sema::{Interpreter, SemaError, Value};

#[test]
fn embedding_prelude_macros_work() {
    let interp = Interpreter::new();
    // Threading macro from the prelude must be available to embedders.
    assert_eq!(
        interp.eval_str("(-> 5 (+ 3) (* 2))").unwrap(),
        Value::int(16)
    );
}

#[test]
fn embedding_defines_persist_across_calls() {
    let interp = Interpreter::new();
    interp.eval_str("(define (sq x) (* x x))").unwrap();
    assert_eq!(interp.eval_str("(sq 9)").unwrap(), Value::int(81));
}

#[test]
fn embedding_preload_module_export_restriction() {
    let interp = Interpreter::new();
    interp
        .preload_module(
            "math",
            r#"(module math (export square)
                 (define (square x) (* x x))
                 (define internal 42))"#,
        )
        .unwrap();
    interp.eval_str(r#"(import "math")"#).unwrap();
    assert_eq!(interp.eval_str("(square 5)").unwrap(), Value::int(25));
    // The non-exported `internal` must not leak to the importer.
    assert!(
        interp.eval_str("internal").is_err(),
        "non-exported binding `internal` must not be visible after import"
    );
}

#[test]
fn embedding_async_works_on_vm() {
    // Async is VM-only; the embedding API runs on the VM, so this must succeed.
    let interp = Interpreter::new();
    assert_eq!(
        interp.eval_str("(await (async (+ 40 2)))").unwrap(),
        Value::int(42)
    );
}

#[test]
fn embedding_async_all_and_channels() {
    // Exercise the scheduler + channels through the embedding API.
    let interp = Interpreter::new();
    let v = interp
        .eval_str("(foldl + 0 (async/all (map (fn (x) (async (* x x))) (list 1 2 3 4))))")
        .unwrap();
    assert_eq!(v, Value::int(30)); // 1+4+9+16

    let v = interp
        .eval_str(
            r#"(let ((ch (channel/new 4)))
                 (channel/send ch 10)
                 (channel/send ch 32)
                 (+ (channel/recv ch) (channel/recv ch)))"#,
        )
        .unwrap();
    assert_eq!(v, Value::int(42));
}

#[test]
fn embedding_register_fn_is_callable() {
    let interp = Interpreter::new();
    interp.register_fn("triple", |args: &[Value]| {
        let n = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        Ok(Value::int(n * 3))
    });
    assert_eq!(interp.eval_str("(triple 14)").unwrap(), Value::int(42));
}

#[test]
fn embedding_register_fn_works_through_hof_callback() {
    // A Rust-registered function must be usable as the callback of a stdlib
    // higher-order function — this routes through the VM's call_callback path.
    let interp = Interpreter::new();
    interp.register_fn("inc", |args: &[Value]| {
        let n = args[0]
            .as_int()
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))?;
        Ok(Value::int(n + 1))
    });
    assert_eq!(
        interp.eval_str("(map inc (list 1 2 3))").unwrap(),
        interp.eval_str("(list 2 3 4)").unwrap()
    );
}

#[test]
fn embedding_register_fn_error_propagates() {
    let interp = Interpreter::new();
    interp.register_fn("must-be-int", |args: &[Value]| {
        args[0]
            .as_int()
            .map(Value::int)
            .ok_or_else(|| SemaError::type_error("integer", args[0].type_name()))
    });
    // Passing a string must surface the Rust-side error as an eval error.
    assert!(interp.eval_str(r#"(must-be-int "nope")"#).is_err());
}

#[test]
fn embedding_eval_parsed_value() {
    // The `eval(&Value)` path (a pre-parsed expression) must behave like `eval_str`.
    let interp = Interpreter::new();
    let parsed = sema_reader::read_many("(* 7 6)").unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(interp.eval(&parsed[0]).unwrap(), Value::int(42));
}

#[test]
fn embedding_global_env_binding_visible_from_sema() {
    let interp = Interpreter::new();
    // Inject a binding directly into the global env from Rust.
    interp.global_env().set_str("injected", Value::int(99));
    assert_eq!(interp.eval_str("injected").unwrap(), Value::int(99));
}

#[test]
fn embedding_without_stdlib_is_minimal() {
    // A no-stdlib interpreter can still do core arithmetic (an intrinsic) but
    // lacks stdlib functions like `map`.
    let interp = Interpreter::builder()
        .without_stdlib()
        .without_llm()
        .build();
    assert_eq!(interp.eval_str("(+ 1 2)").unwrap(), Value::int(3));
    assert!(
        interp.eval_str("(map (fn (x) x) (list 1 2))").is_err(),
        "stdlib `map` must be absent when stdlib is disabled"
    );
}

#[test]
fn embedding_without_llm_keeps_stdlib() {
    let interp = Interpreter::builder().without_llm().build();
    assert_eq!(
        interp.eval_str(r#"(string-upcase "hi")"#).unwrap(),
        interp.eval_str(r#""HI""#).unwrap()
    );
}

#[test]
fn embedding_load_file_roundtrips() {
    use std::io::Write;
    let interp = Interpreter::new();
    let dir = std::env::temp_dir();
    let path = dir.join(format!("sema_embed_test_{}.sema", std::process::id()));
    {
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "(define (cube x) (* x x x))").unwrap();
    }
    interp.load_file(&path).unwrap();
    assert_eq!(interp.eval_str("(cube 3)").unwrap(), Value::int(27));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn embedding_load_file_resolves_relative_modules_and_macro_loads() {
    let interp = Interpreter::new();
    let dir = std::env::temp_dir().join(format!("sema_embed_relative_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helper.sema"),
        "(module helper (export answer) (define answer 41))",
    )
    .unwrap();
    std::fs::write(dir.join("macros.sema"), "(defmacro add-one (x) `(+ ,x 1))").unwrap();
    std::fs::write(
        dir.join("main.sema"),
        "(import \"helper.sema\") (load \"macros.sema\") (define result (add-one answer))",
    )
    .unwrap();
    interp.load_file(dir.join("main.sema")).unwrap();
    assert_eq!(interp.eval_str("result").unwrap(), Value::int(42));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn embedding_dynamic_top_level_load_keeps_runtime_argument_evaluation() {
    let interp = Interpreter::new();
    let dir = std::env::temp_dir().join(format!("sema_embed_dynamic_load_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("loaded.sema"), "(define loaded-answer 42)").unwrap();
    std::fs::write(
        dir.join("main.sema"),
        "(define loaded-path \"loaded.sema\") (load loaded-path) (define result loaded-answer)",
    )
    .unwrap();

    interp.load_file(dir.join("main.sema")).unwrap();
    assert_eq!(interp.eval_str("result").unwrap(), Value::int(42));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bytecode_file_resolves_relative_imports() {
    let interp = Interpreter::new();
    let dir = std::env::temp_dir().join(format!("sema_bytecode_relative_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helper.sema"),
        "(module helper (export bytecode-answer) (define bytecode-answer 42))",
    )
    .unwrap();
    let compiler = sema_eval::Interpreter::new();
    let compiled = compiler
        .compile_to_bytecode("(import \"helper.sema\") bytecode-answer")
        .unwrap();
    let bytes = sema_vm::serialize_to_bytes(&compiled, 0).unwrap();
    let path = dir.join("main.semac");
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        interp.run_bytecode_file(&path, &bytes).unwrap(),
        Value::int(42)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn loaded_module_does_not_replace_importing_module_exports() {
    let interp = Interpreter::new();
    let dir = std::env::temp_dir().join(format!("sema_nested_exports_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("inner.sema"),
        "(module inner (export hidden) (define hidden 1))",
    )
    .unwrap();
    std::fs::write(
        dir.join("outer.sema"),
        "(module outer (export public) (load \"inner.sema\") (define public 42) (define private 9))",
    )
    .unwrap();
    let entry = dir.join("entry.sema");
    std::fs::write(&entry, "(import \"outer.sema\")").unwrap();
    interp.load_file(&entry).unwrap();
    assert_eq!(interp.eval_str("public").unwrap(), Value::int(42));
    assert!(interp.eval_str("private").is_err());
    assert!(interp.eval_str("hidden").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn embedding_preload_module_all_bindings_when_no_export() {
    // With no `(export ...)`, every top-level binding is importable.
    let interp = Interpreter::new();
    interp
        .preload_module("util", "(define (double x) (* x 2))\n(define answer 42)")
        .unwrap();
    interp.eval_str(r#"(import "util")"#).unwrap();
    assert_eq!(interp.eval_str("(double 21)").unwrap(), Value::int(42));
    assert_eq!(interp.eval_str("answer").unwrap(), Value::int(42));
}
