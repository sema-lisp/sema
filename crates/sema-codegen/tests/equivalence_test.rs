//! The correctness oracle for native code generation.
//!
//! Every case runs the same Sema source twice in the same process: once with
//! the VM alone, then again with the Cranelift backend installed and the
//! compile threshold at zero so every eligible function is compiled on its
//! first call. The two results must be identical, including their type — a
//! fixnum that should have become a bignum, or a float that should have stayed
//! a float, fails here.
//!
//! Running both halves in one process also proves the interesting property: a
//! bail leaves nothing behind. The compiled call abandoned halfway and the VM
//! re-run must agree with the VM-only answer.

use sema_eval::Interpreter;

/// Run `src` on the VM alone.
fn vm_only(src: &str) -> String {
    sema_codegen::uninstall();
    let interp = Interpreter::new();
    match interp.eval_str(src) {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("error: {e}"),
    }
}

/// Run `src` with the JIT installed and compiling eagerly.
fn with_jit(src: &str) -> String {
    sema_codegen::install().expect("no code generator for this host");
    // Compile on the first call so a test does not have to loop 32 times to
    // reach the compiled path.
    sema_vm::jit::set_threshold(0);
    sema_vm::jit::reset_stats();
    let interp = Interpreter::new();
    let out = match interp.eval_str(src) {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("error: {e}"),
    };
    sema_codegen::uninstall();
    out
}

#[track_caller]
fn same(src: &str) -> String {
    let expected = vm_only(src);
    let actual = with_jit(src);
    assert_eq!(
        expected, actual,
        "VM and JIT disagree on:\n{src}\n  vm  = {expected}\n  jit = {actual}"
    );
    expected
}

#[track_caller]
fn same_is(src: &str, expected: &str) {
    let got = same(src);
    assert_eq!(got, expected, "unexpected result for:\n{src}");
}

// ── arithmetic ────────────────────────────────────────────────────

#[test]
fn integer_arithmetic() {
    same_is("(define (f a b) (+ a b)) (f 2 3)", "Int(5)");
    same_is("(define (f a b) (- a b)) (f 2 3)", "Int(-1)");
    same_is("(define (f a b) (* a b)) (f 6 7)", "Int(42)");
    same_is("(define (f a b) (/ a b)) (f 6 3)", "Int(2)");
    same_is("(define (f a) (- a)) (f 9)", "Int(-9)");
}

#[test]
fn float_arithmetic() {
    same_is("(define (f a b) (+ a b)) (f 1.5 2.25)", "Float(3.75)");
    same_is("(define (f a b) (* a b)) (f 1.5 2.0)", "Float(3)");
    same_is("(define (f a b) (/ a b)) (f 1.0 4.0)", "Float(0.25)");
    same_is("(define (f a) (- a)) (f 2.5)", "Float(-2.5)");
}

#[test]
fn mixed_int_and_float_promote() {
    same_is("(define (f a b) (+ a b)) (f 1 2.5)", "Float(3.5)");
    same_is("(define (f a b) (- a b)) (f 1.5 1)", "Float(0.5)");
    same_is("(define (f a b) (* a b)) (f 2 0.5)", "Float(1)");
    same_is("(define (f a b) (/ a b)) (f 1 2.0)", "Float(0.5)");
}

/// Division that is not exact yields a rational, which compiled code cannot
/// build — it must bail and let the VM produce the exact value.
#[test]
fn inexact_integer_division_bails_to_the_vm() {
    same_is("(define (f a b) (/ a b)) (f 1 3)", "Rational(1/3)");
    same_is("(define (f a b) (/ a b)) (f 7 2)", "Rational(7/2)");
}

#[test]
fn division_by_zero_raises_through_the_vm() {
    let out = same("(define (f a b) (/ a b)) (f 1 0)");
    assert!(out.contains("division by zero"), "unexpected: {out}");
}

/// Sema's `modulo` is floored, not truncated.
#[test]
fn modulo_is_floored() {
    same_is("(define (f a b) (modulo a b)) (f 7 3)", "Int(1)");
    same_is("(define (f a b) (modulo a b)) (f -7 3)", "Int(2)");
    same_is("(define (f a b) (modulo a b)) (f 7 -3)", "Int(-2)");
    same_is("(define (f a b) (modulo a b)) (f -7 -3)", "Int(-1)");
}

#[test]
fn modulo_by_zero_raises_through_the_vm() {
    let out = same("(define (f a b) (modulo a b)) (f 5 0)");
    assert!(out.contains("modulo by zero"), "unexpected: {out}");
}

// ── the fixnum boundary ───────────────────────────────────────────

/// A fixnum holds 45 bits. Arithmetic that leaves that range promotes to a
/// bignum in the VM, and compiled code must bail rather than truncate.
#[test]
fn fixnum_overflow_promotes_to_bignum() {
    // 2^44 - 1 is the largest fixnum.
    same_is(
        "(define (f a b) (+ a b)) (f 17592186044415 1)",
        "Int(17592186044416)",
    );
    same_is(
        "(define (f a b) (- a b)) (f -17592186044416 1)",
        "Int(-17592186044417)",
    );
    same_is(
        "(define (f a b) (* a b)) (f 17592186044415 17592186044415)",
        "Int(309485009821309884352692225)",
    );
    // Negating the smallest fixnum leaves the range too.
    same_is(
        "(define (f a) (- a)) (f -17592186044416)",
        "Int(17592186044416)",
    );
}

#[test]
fn fixnum_boundary_values_stay_fixnums() {
    same_is(
        "(define (f a b) (+ a b)) (f 17592186044414 1)",
        "Int(17592186044415)",
    );
    same_is(
        "(define (f a b) (- a b)) (f -17592186044415 1)",
        "Int(-17592186044416)",
    );
}

/// A product whose true value exceeds `i64` must not be trusted from the low
/// half alone.
#[test]
fn wide_products_do_not_wrap() {
    same_is(
        "(define (f a b) (* a b)) (f 17592186044415 8796093022207)",
        "Int(154742504910646146083323905)",
    );
    same_is(
        "(define (f a b) (* a b)) (f -17592186044416 17592186044415)",
        "Int(-309485009821327476538736640)",
    );
}

// ── comparison, truthiness, control flow ──────────────────────────

#[test]
fn numeric_comparison() {
    same_is("(define (f a b) (< a b)) (f 1 2)", "Bool(true)");
    same_is("(define (f a b) (< a b)) (f 2 1)", "Bool(false)");
    same_is("(define (f a b) (> a b)) (f 2 1)", "Bool(true)");
    same_is("(define (f a b) (<= a b)) (f 2 2)", "Bool(true)");
    same_is("(define (f a b) (>= a b)) (f 1 2)", "Bool(false)");
    same_is("(define (f a b) (= a b)) (f 3 3)", "Bool(true)");
}

/// A 45-bit int converts to `f64` exactly, so mixed comparison stays faithful.
#[test]
fn mixed_comparison_is_exact() {
    same_is("(define (f a b) (< a b)) (f 1 1.5)", "Bool(true)");
    same_is("(define (f a b) (= a b)) (f 2 2.0)", "Bool(true)");
    same_is(
        "(define (f a b) (< a b)) (f 17592186044415 1.0e30)",
        "Bool(true)",
    );
    same_is(
        "(define (f a b) (= a b)) (f 17592186044415 17592186044415.0)",
        "Bool(true)",
    );
}

/// NaN is unordered and never equal to itself, so `=` and `<` are both false.
///
/// `>=` and `<=` are the interesting ones: the VM builds them as `!(a < b)` and
/// `!(b < a)`, which makes them answer `#t` for a NaN operand rather than the
/// `#f` the IEEE predicates give. Compiled code has to reproduce the VM's
/// answer, not IEEE's.
#[test]
fn nan_comparisons_match_the_vm() {
    same_is(
        "(define (f a b) (= a b)) (f (/ 0.0 0.0) (/ 0.0 0.0))",
        "Bool(false)",
    );
    same_is(
        "(define (f a b) (< a b)) (f (/ 0.0 0.0) 1.0)",
        "Bool(false)",
    );
    same_is(
        "(define (f a b) (> a b)) (f (/ 0.0 0.0) 1.0)",
        "Bool(false)",
    );
    same_is(
        "(define (f a b) (>= a b)) (f (/ 0.0 0.0) 1.0)",
        "Bool(true)",
    );
    same_is(
        "(define (f a b) (<= a b)) (f (/ 0.0 0.0) 1.0)",
        "Bool(true)",
    );
}

/// Hardware's default NaN has the same bits as nil. A compiled float operation
/// that produced one without canonicalizing would return nil here.
#[test]
fn nan_result_is_a_float_not_nil() {
    same_is("(define (f a b) (/ a b)) (f 0.0 0.0)", "Float(NaN)");
    same_is(
        "(define (f a b) (- a b)) (f (/ 1.0 0.0) (/ 1.0 0.0))",
        "Float(NaN)",
    );
    same_is("(define (f a b) (* a b)) (f (/ 1.0 0.0) 0.0)", "Float(NaN)");
}

#[test]
fn infinities_survive() {
    same_is("(define (f a b) (/ a b)) (f 1.0 0.0)", "Float(inf)");
    same_is("(define (f a b) (/ a b)) (f -1.0 0.0)", "Float(-inf)");
}

#[test]
fn conditionals_and_truthiness() {
    same_is("(define (f a) (if a 1 2)) (f #t)", "Int(1)");
    same_is("(define (f a) (if a 1 2)) (f #f)", "Int(2)");
    same_is("(define (f a) (if a 1 2)) (f nil)", "Int(2)");
    // 0 and the empty-ish values are truthy in Sema.
    same_is("(define (f a) (if a 1 2)) (f 0)", "Int(1)");
    same_is("(define (f a) (not a)) (f #f)", "Bool(true)");
    same_is("(define (f a) (not a)) (f 0)", "Bool(false)");
    same_is("(define (f a) (not a)) (f nil)", "Bool(true)");
}

#[test]
fn short_circuit_operators() {
    same_is("(define (f a b) (and a b)) (f #t 5)", "Int(5)");
    same_is("(define (f a b) (and a b)) (f #f 5)", "Bool(false)");
    same_is("(define (f a b) (or a b)) (f #f 7)", "Int(7)");
    same_is("(define (f a b) (or a b)) (f 3 7)", "Int(3)");
}

#[test]
fn let_bindings() {
    same_is(
        "(define (f a) (let ((x (* a 2)) (y 3)) (+ x y))) (f 5)",
        "Int(13)",
    );
    same_is(
        "(define (f a) (let* ((x (+ a 1)) (y (* x 2))) (- y a))) (f 4)",
        "Int(6)",
    );
}

// ── recursion ─────────────────────────────────────────────────────

#[test]
fn self_tail_recursion_becomes_a_loop() {
    same_is(
        "(define (sum n acc) (if (= n 0) acc (sum (- n 1) (+ acc n)))) (sum 100000 0)",
        "Int(5000050000)",
    );
}

#[test]
fn non_tail_self_recursion() {
    same_is(
        "(define (fib n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2))))) (fib 25)",
        "Int(75025)",
    );
    same_is(
        "(define (fact n) (if (= n 0) 1 (* n (fact (- n 1))))) (fact 15)",
        "Int(1307674368000)",
    );
}

/// A factorial that outgrows the fixnum range has to reach the same bignum the
/// VM would produce, which means bailing out of an already-deep native
/// recursion.
#[test]
fn recursion_that_overflows_mid_way_bails_correctly() {
    same_is(
        "(define (fact n) (if (= n 0) 1 (* n (fact (- n 1))))) (fact 25)",
        "Int(15511210043330985984000000)",
    );
}

/// Compiled code recurses on the machine stack, which the VM cannot police, so
/// it counts depth itself and bails. The VM then raises its usual error rather
/// than the process crashing.
#[test]
fn unbounded_non_tail_recursion_raises_instead_of_crashing() {
    let out = same("(define (f n) (+ 1 (f (- n 1)))) (f 1000000)");
    assert!(out.contains("stack overflow"), "unexpected: {out}");
}

/// A named let is a self-tail-recursive loop; this is the shape numeric code
/// most often takes.
#[test]
fn named_let_loop() {
    same_is(
        "(let loop ((i 0) (acc 0)) (if (= i 1000) acc (loop (+ i 1) (+ acc (* i i)))))",
        "Int(332833500)",
    );
}

// ── values the compiled subset must refuse ────────────────────────

/// A non-immediate argument must never reach compiled code: it owns an `Rc`,
/// and compiled code does no refcount work.
#[test]
fn heap_arguments_go_to_the_vm() {
    same_is("(define (f a b) (+ a b)) (f \"x\" \"y\")", "String(\"xy\")");
    same_is("(define (f a) (if a 1 2)) (f (list 1 2))", "Int(1)");
    same_is("(define (f a b) (< a b)) (f \"a\" \"b\")", "Bool(true)");
}

#[test]
fn non_numeric_arithmetic_still_raises() {
    let out = same("(define (f a b) (- a b)) (f #t 1)");
    assert!(out.contains("error"), "unexpected: {out}");
}

#[test]
fn functions_calling_other_functions_run_in_the_vm() {
    same_is(
        "(define (g x) (* x 2)) (define (f x) (+ (g x) 1)) (f 20)",
        "Int(41)",
    );
}

// ── the JIT actually ran ──────────────────────────────────────────

/// The equivalence tests above would all still pass if the backend silently
/// compiled nothing. This one proves compiled code really executed.
#[test]
fn hot_functions_are_compiled_and_executed() {
    sema_codegen::install().expect("no code generator for this host");
    sema_vm::jit::set_threshold(0);
    sema_vm::jit::reset_stats();

    let interp = Interpreter::new();
    let result = interp
        .eval_str("(define (sq x) (* x x)) (let loop ((i 0) (acc 0)) (if (= i 200) acc (loop (+ i 1) (+ acc (sq i)))))")
        .expect("program failed");
    assert_eq!(format!("{result:?}"), "Int(2646700)");

    let stats = sema_vm::jit::stats();
    sema_codegen::uninstall();

    assert!(stats.compiled > 0, "nothing was compiled: {stats:?}");
    assert!(
        stats.native_calls > 0,
        "no compiled call completed: {stats:?}"
    );
}

/// The compile threshold must actually hold work back.
#[test]
fn cold_functions_are_not_compiled() {
    sema_codegen::install().expect("no code generator for this host");
    sema_vm::jit::set_threshold(1_000_000);
    sema_vm::jit::reset_stats();

    let interp = Interpreter::new();
    let result = interp
        .eval_str("(define (sq x) (* x x)) (sq 7)")
        .expect("program failed");
    assert_eq!(format!("{result:?}"), "Int(49)");

    let stats = sema_vm::jit::stats();
    sema_vm::jit::set_threshold(0);
    sema_codegen::uninstall();

    assert_eq!(stats.compiled, 0, "compiled below the threshold: {stats:?}");
    assert_eq!(stats.native_calls, 0, "ran native code: {stats:?}");
}

/// Uninstalling must stop compiled code from running, because the backend that
/// owns its memory is about to be dropped.
#[test]
fn uninstalling_stops_native_execution() {
    sema_codegen::install().expect("no code generator for this host");
    sema_vm::jit::set_threshold(0);
    let interp = Interpreter::new();
    interp.eval_str("(define (sq x) (* x x)) (sq 3)").unwrap();

    sema_codegen::uninstall();
    assert!(!sema_codegen::is_installed());

    sema_vm::jit::reset_stats();
    let result = interp.eval_str("(sq 4)").expect("program failed");
    assert_eq!(format!("{result:?}"), "Int(16)");
    assert_eq!(sema_vm::jit::stats().native_calls, 0);
}
