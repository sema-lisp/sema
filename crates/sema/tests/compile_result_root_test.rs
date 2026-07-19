//! Host-root coverage for deserialized `.semac` programs.

use std::rc::Rc;
use std::time::Duration;

use sema_core::{intern, Value};
use sema_eval::{execute_compile_result, Interpreter};
use sema_vm::runtime::{OutputEvent, RootOptions, RootPoll};
use sema_vm::{deserialize_from_bytes, serialize_to_bytes, CompileResult};

fn deserialized(interpreter: &Interpreter, source: &str) -> CompileResult {
    let compiled = interpreter
        .compile_to_bytecode(source)
        .expect("source compiles");
    let bytes = serialize_to_bytes(&compiled, 0).expect("bytecode serializes");
    deserialize_from_bytes(&bytes).expect("bytecode deserializes")
}

#[test]
fn submit_compile_result_stays_pending_until_the_host_drives_it() {
    let interpreter = Interpreter::new();
    let result = deserialized(
        &interpreter,
        r#"
            (define compiled-root-value 40)
            (println "compiled-root-output")
            (async/sleep 1)
            (+ compiled-root-value 2)
        "#,
    );

    let root = interpreter
        .submit_compile_result(
            result,
            RootOptions {
                name: Some("compiled-root".into()),
                capture_output: true,
            },
        )
        .expect("compiled root submits");

    std::thread::sleep(Duration::from_millis(5));
    assert!(matches!(root.poll_result(), RootPoll::Pending));
    assert!(
        interpreter
            .global_env
            .get(intern("compiled-root-value"))
            .is_none(),
        "submission must not execute top-level definitions"
    );
    assert!(
        interpreter.take_output().is_empty(),
        "submission must not execute output side effects"
    );

    let value = interpreter
        .drive_until_settled(&root)
        .expect("host driving settles the compiled root");
    assert_eq!(value, Value::int(42));
    assert_eq!(
        interpreter.global_env.get(intern("compiled-root-value")),
        Some(Value::int(40)),
        "compiled definitions must land in the interpreter globals"
    );
    assert!(interpreter.take_output().iter().any(|event| {
        matches!(
            event,
            OutputEvent::Stdout { root: event_root, text }
                if *event_root == root.id() && text.contains("compiled-root-output")
        )
    }));
}

#[test]
fn submit_compile_result_preserves_execute_error_semantics() {
    let interpreter = Interpreter::new();

    let mut synchronous = deserialized(&interpreter, "(+ 1 2)");
    synchronous.chunk.code.clear();
    let synchronous_error = execute_compile_result(
        &interpreter.ctx,
        Rc::clone(&interpreter.global_env),
        synchronous,
    )
    .expect_err("invalid bytecode must fail synchronous execution");

    let mut submitted = deserialized(&interpreter, "(+ 1 2)");
    submitted.chunk.code.clear();
    let root = interpreter
        .submit_compile_result(submitted, RootOptions::default())
        .expect("building the root remains non-driving");
    assert!(matches!(root.poll_result(), RootPoll::Pending));
    let submitted_error = interpreter
        .drive_until_settled(&root)
        .expect_err("invalid bytecode must fail runtime execution");

    assert_eq!(submitted_error.to_string(), synchronous_error.to_string());
}

#[test]
fn execute_compile_result_remains_a_synchronous_nested_entry() {
    let interpreter = Interpreter::new();
    let result = deserialized(
        &interpreter,
        "(define nested-bytecode-value 7) (+ nested-bytecode-value 2)",
    );

    let value =
        execute_compile_result(&interpreter.ctx, Rc::clone(&interpreter.global_env), result)
            .expect("nested bytecode execution remains synchronous");

    assert_eq!(value, Value::int(9));
    assert_eq!(
        interpreter.global_env.get(intern("nested-bytecode-value")),
        Some(Value::int(7))
    );
}
