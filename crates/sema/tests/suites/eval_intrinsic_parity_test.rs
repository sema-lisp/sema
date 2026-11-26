//! Intrinsic/native parity.
//!
//! The compiler turns some global calls into inline opcodes
//! (`try_compile_intrinsic` in `crates/sema-vm/src/compiler.rs`). An indirect
//! call to the same function (`apply`, a call through a local) runs the
//! stdlib native instead. Both forms must return the same value or the same
//! error for every input. The intrinsic opcode owns only its fast success
//! paths; on any other input it defers to the native
//! (`intrinsic_fallback` in `crates/sema-vm/src/vm.rs`).

use sema_eval::Interpreter;

/// `(name, args)` pairs. `args` is spliced as `(name args)` and as
/// `(apply name (list args))`. Every name in the compiler's intrinsic table
/// must appear here; `every_intrinsic_has_a_parity_case` enforces that.
const CASES: &[(&str, &str)] = &[
    ("not", "1"),
    ("not", "nil"),
    ("-", "\"a\""),
    ("-", "-9223372036854775808"),
    ("+", "1 2"),
    ("+", "\"a\" 1"),
    ("+", "\"a\" \"b\""),
    ("+", "9223372036854775807 1"),
    ("+", "1.5 1/2"),
    ("-", "\"a\" 1"),
    ("-", "-9223372036854775808 1"),
    ("*", "\"a\" 2"),
    ("*", "9223372036854775807 2"),
    ("/", "1 0"),
    ("/", "1 2"),
    ("/", "\"a\" 2"),
    ("<", "1 \"a\""),
    ("<", "1 2.5"),
    (">", "\"a\" 1"),
    ("<=", "nil 1"),
    (">=", "\"a\" 1"),
    ("=", "1 1.0"),
    ("=", "\"a\" 1"),
    ("car", "5"),
    ("car", "'()"),
    ("first", "5"),
    ("first", "[1 2]"),
    ("cdr", "5"),
    ("rest", "\"ab\""),
    ("cons", "1 2"),
    ("cons", "1 '(2)"),
    ("null?", "'()"),
    ("pair?", "5"),
    ("list?", "nil"),
    ("number?", "1/2"),
    ("string?", "\"a\""),
    ("symbol?", "'a"),
    ("length", "5"),
    ("length", "\"é\""),
    ("append", "'(1) 2"),
    ("append", "1 '(2)"),
    ("append", "[1] '(2)"),
    ("get", "5 :a"),
    ("get", "{:a 1} :a"),
    ("get", "[1 2] 0"),
    ("contains?", "5 1"),
    ("contains?", "[1 2] 0"),
    ("contains?", "'(1 2) 0"),
    ("get", "'(1 2) 0"),
    ("+", "1 \"x\""),
    ("mod", "5 0"),
    ("mod", "-7 2"),
    ("mod", "\"a\" 2"),
    ("modulo", "5 0"),
    ("modulo", "-7 2"),
    ("nth", "'(1 2) 5"),
    ("nth", "5 0"),
    ("nth", "'(1 2) -1"),
    ("nth", "1 '(1 2)"),
    ("nth", "[1 2] 1"),
    ("mutable-array/get", "5 0"),
    ("mutable-array/get", "(mutable-array/new 2 7) 9"),
    ("mutable-array/set!", "5 0 1"),
    ("string-length", "42"),
    ("string-length", "\"héllo\""),
    ("string-ref", "42 1"),
    ("string-ref", "\"abc\" \"x\""),
    ("string-ref", "\"abc\" 9"),
    ("string-ref", "\"abc\" -1"),
    ("string-ref", "\"héllo\" 1"),
    ("string-append", "\"a\" 1"),
    ("string-append", "\"a\" \"b\""),
];

/// Value, or error text plus hint, so an `Ok` and an `Err` never compare
/// equal. The hint is a separate field that `Display` does not print.
fn run(src: &str) -> Result<String, String> {
    Interpreter::new()
        .eval_str_compiled(src)
        .map(|v| format!("{v:?}"))
        .map_err(|e| format!("{e} | hint: {:?}", e.hint()))
}

#[test]
fn intrinsic_call_matches_apply() {
    let mut diffs = Vec::new();
    for (name, args) in CASES {
        let direct = format!("({name} {args})");
        let indirect = format!("(apply {name} (list {args}))");
        let (d, a) = (run(&direct), run(&indirect));
        if d != a {
            diffs.push(format!("{direct}\n  direct: {d:?}\n  apply:  {a:?}"));
        }
    }
    assert!(
        diffs.is_empty(),
        "intrinsic/native divergence:\n{}",
        diffs.join("\n")
    );
}

/// The intrinsic call also matches a call through a local binding, which
/// the compiler cannot intrinsify.
#[test]
fn intrinsic_call_matches_local_binding_call() {
    let mut diffs = Vec::new();
    for (name, args) in CASES {
        let direct = format!("({name} {args})");
        let through_local = format!("(let ((f {name})) (f {args}))");
        let (d, l) = (run(&direct), run(&through_local));
        if d != l {
            diffs.push(format!("{direct}\n  direct: {d:?}\n  local:  {l:?}"));
        }
    }
    assert!(
        diffs.is_empty(),
        "intrinsic/native divergence:\n{}",
        diffs.join("\n")
    );
}

#[test]
fn intrinsic_error_is_catchable_and_execution_continues() {
    let v = Interpreter::new()
        .eval_str_compiled(
            "(define (f s) (string-ref s 0))
             (list (try (f 42) (catch e (:message e))) (f \"xyz\"))",
        )
        .expect("eval");
    assert_eq!(
        format!("{v:?}"),
        format!(
            "{:?}",
            Interpreter::new()
                .eval_str_compiled("(list \"string-ref argument 1 expected string, got int\" #\\x)")
                .unwrap()
        )
    );
}

/// Without stdlib there is no native to defer to; the opcode raises its own
/// error instead of panicking or returning a wrong value.
#[test]
fn intrinsic_error_without_stdlib_uses_vm_error() {
    let interp = sema::Interpreter::builder().with_stdlib(false).build();
    let err = interp.eval_str("(string-ref 42 1)").unwrap_err();
    assert!(err.to_string().contains("expected string"), "{err}");
}

/// Guard: a new entry in the compiler's intrinsic table needs a parity case.
#[test]
fn every_intrinsic_has_a_parity_case() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../sema-vm/src/compiler.rs"
    ))
    .expect("read compiler.rs");
    let start = src
        .find("fn try_compile_intrinsic")
        .expect("intrinsic table");
    let end = start
        + src[start..]
            .find("_ => return Ok(false)")
            .expect("table end");
    let mut missing = Vec::new();
    for part in src[start..end].split("(\"").skip(1) {
        let name = &part[..part.find('"').unwrap()];
        if !CASES.iter().any(|(n, _)| *n == name) && !missing.contains(&name) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "intrinsics without a parity case: {missing:?}"
    );
}
