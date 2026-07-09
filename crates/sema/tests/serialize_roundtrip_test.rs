//! End-to-end serialize round-trip tests for the bytecode format.
//!
//! Each test: compile → serialize → deserialize → run in VM → compare to direct eval.

mod common;

use sema_core::Value;
use sema_eval::Interpreter;
use sema_vm::{deserialize_from_bytes, serialize_to_bytes, Closure, Function, VM};
use std::rc::Rc;

/// Compile source, serialize to bytes, deserialize back, execute, and return the result.
fn eval_roundtrip(input: &str) -> Value {
    let interp = Interpreter::new();

    // 1. Compile to bytecode
    let compiled = interp
        .compile_to_bytecode(input)
        .unwrap_or_else(|e| panic!("compile failed for `{input}`: {e}"));

    // 2. Serialize to bytes
    let bytes = serialize_to_bytes(&compiled, 0)
        .unwrap_or_else(|e| panic!("serialize failed for `{input}`: {e}"));

    // 3. Deserialize back
    let deserialized = deserialize_from_bytes(&bytes)
        .unwrap_or_else(|e| panic!("deserialize failed for `{input}`: {e}"));

    // 4. Execute the deserialized program
    let functions: Vec<Rc<Function>> = deserialized.functions.into_iter().map(Rc::new).collect();
    let main_cache_slots = deserialized.chunk.n_global_cache_slots;
    let closure = Rc::new(Closure {
        func: Rc::new(Function {
            name: None,
            chunk: deserialized.chunk,
            upvalue_descs: Vec::new(),
            upvalue_names: Vec::new(),
            arity: 0,
            has_rest: false,
            param_names: Vec::new().into(),
            local_names: Vec::new(),
            local_scopes: Vec::new(),
            source_file: None,
            cache_offset: 0,
        }),
        upvalues: Vec::new(),
        globals: None,
        functions: None,
    });
    let mut vm = VM::new(interp.global_env.clone(), functions, &[], main_cache_slots).unwrap();
    vm.execute(closure, &interp.ctx).unwrap_or_else(|e| {
        panic!("VM execution of deserialized program failed for `{input}`: {e}")
    })
}

/// Assert that the serialize→deserialize→execute result matches a direct eval.
fn assert_roundtrip(input: &str) {
    let direct = common::eval(input);
    let rt_result = eval_roundtrip(input);
    assert_eq!(
        rt_result, direct,
        "roundtrip mismatch for `{input}`:\n  roundtrip: {rt_result:?}\n  direct:    {direct:?}"
    );
}

fn assert_roundtrip_eq(input: &str, expected: Value) {
    let rt_result = eval_roundtrip(input);
    assert_eq!(
        rt_result, expected,
        "roundtrip mismatch for `{input}`:\n  roundtrip: {rt_result:?}\n  expected:  {expected:?}"
    );
}

// ============================================================
// 1. Closure with upvalue mutation
// ============================================================

#[test]
fn roundtrip_closure_upvalue_mutation() {
    assert_roundtrip_eq(
        "(begin (define (make-counter) (let ((n 0)) (fn () (set! n (+ n 1)) n))) (define c (make-counter)) (c) (c) (c))",
        Value::int(3),
    );
}

// ============================================================
// 2. Nested closures
// ============================================================

#[test]
fn roundtrip_nested_closures() {
    assert_roundtrip("(begin (define (f x) (fn (y) (fn (z) (list x y z)))) (((f 1) 2) 3))");
}

// ============================================================
// 3. Try/catch across function boundary
// ============================================================

#[test]
fn roundtrip_try_catch_across_function() {
    assert_roundtrip_eq(
        r#"(begin (define (g) (throw {:msg "x"})) (try (g) (catch e (get (get e :value) :msg))))"#,
        Value::string("x"),
    );
}

// ============================================================
// 4. Match with quoted literal
// ============================================================

#[test]
fn roundtrip_match_quoted_literal() {
    assert_roundtrip_eq("(match 'a ('a 1) (_ 0))", Value::int(1));
}

// ============================================================
// 5. Upvalue + do loop + closure calls
// ============================================================

#[test]
fn roundtrip_upvalue_do_loop() {
    assert_roundtrip_eq(
        "(begin (define (mk) (let ((x 0)) (fn () (set! x (+ x 1)) x))) (define c (mk)) (do ((i 0 (+ i 1)) (acc 0 (+ acc (c)))) ((= i 3) acc)))",
        Value::int(6),
    );
}

// ============================================================
// 6. String constant reuse (string table)
// ============================================================

#[test]
fn roundtrip_string_constant_reuse() {
    assert_roundtrip(
        r#"(list (string-length "hello") (string-length "hello") (string-append "hello" " " "world"))"#,
    );
}

// ============================================================
// 7. Map constants
// ============================================================

#[test]
fn roundtrip_map_constants() {
    assert_roundtrip_eq("(get {:a 1 :b 2 :c 3} :b)", Value::int(2));
}

// ============================================================
// 8. Deeply nested quoted structure
// ============================================================

#[test]
fn roundtrip_deeply_nested_quoted() {
    assert_roundtrip("(car (cdr '(1 (2 (3 (4))))))");
}

// ============================================================
// 9. Multiple functions with shared names (constant pool)
// ============================================================

#[test]
fn roundtrip_multiple_functions_shared_names() {
    assert_roundtrip_eq(
        "(begin (define (add a b) (+ a b)) (define (mul a b) (* a b)) (+ (add 3 4) (mul 5 6)))",
        Value::int(37),
    );
}

// ============================================================
// 10. Recursive function (self-reference serialization)
// ============================================================

#[test]
fn roundtrip_recursive_function() {
    assert_roundtrip_eq(
        "(begin (define (fact n) (if (<= n 1) 1 (* n (fact (- n 1))))) (fact 10))",
        Value::int(3628800),
    );
}

// ============================================================
// 11. Letrec mutual recursion
// ============================================================

#[test]
fn roundtrip_letrec_mutual_recursion() {
    assert_roundtrip_eq(
        "(letrec ((even? (fn (n) (if (= n 0) #t (odd? (- n 1))))) (odd? (fn (n) (if (= n 0) #f (even? (- n 1)))))) (even? 10))",
        Value::bool(true),
    );
}

// ============================================================
// 12. Vector and list constants
// ============================================================

#[test]
fn roundtrip_vector_and_list_constants() {
    assert_roundtrip_eq(
        "(begin (define v [1 2 3]) (define l '(4 5 6)) (+ (nth v 1) (nth l 1)))",
        Value::int(7),
    );
}

// ============================================================
// 13. Roundtrip WITHOUT CallNative — design invariant
// ============================================================

/// `compile_to_bytecode` passes `None` for known_natives, so CallNative opcodes
/// are never emitted in serialized bytecode. The native_table is not part of the
/// bytecode format. This test verifies that programs using native functions still
/// work correctly through serialization via the CallGlobal path.
#[test]
fn roundtrip_native_functions_use_call_global() {
    // These all call native functions — in serialized form they use CallGlobal,
    // not CallNative, because known_natives is None during compile_to_bytecode.
    assert_roundtrip("(string-append \"hello\" \" world\")");
    assert_roundtrip("(length '(1 2 3))");
    assert_roundtrip("(not #f)");
    assert_roundtrip("(null? '())");
    assert_roundtrip("(begin (define xs '(1 2 3)) (map (fn (x) (* x 2)) xs))");
}

// ============================================================
// 14. local_scopes block-scope debug metadata survives roundtrip (DAP-6)
// ============================================================

/// `Function::local_scopes` is compile-time debug metadata the debugger uses to
/// hide out-of-scope block locals. It must survive serialization so `.semac`-
/// loaded functions show correct in-scope locals (bytecode format version 4).
#[test]
fn roundtrip_preserves_local_scopes() {
    let interp = Interpreter::new();
    // A function with a `let`-introduced block local produces local_scopes.
    let src = "(define (f a) (let ((b (+ a 1))) (+ a b)))";
    let compiled = interp
        .compile_to_bytecode(src)
        .unwrap_or_else(|e| panic!("compile failed: {e}"));

    // Find the function that has recorded block scopes.
    let before: Vec<Vec<(u16, u32, u32)>> = compiled
        .functions
        .iter()
        .map(|f| f.local_scopes.clone())
        .collect();
    assert!(
        before.iter().any(|s| !s.is_empty()),
        "expected at least one function with recorded local_scopes; got {before:?}"
    );

    let bytes = serialize_to_bytes(&compiled, 0).expect("serialize");
    let deserialized = deserialize_from_bytes(&bytes).expect("deserialize");

    let after: Vec<Vec<(u16, u32, u32)>> = deserialized
        .functions
        .iter()
        .map(|f| f.local_scopes.clone())
        .collect();

    assert_eq!(
        before, after,
        "local_scopes must be identical after serialize/deserialize roundtrip"
    );
}
