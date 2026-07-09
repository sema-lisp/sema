mod common;

use sema_core::Value;
use std::collections::BTreeMap;

// ============================================================
// Arithmetic & Math
// ============================================================

eval_tests! {
    arith_add: "(+ 1 2)" => Value::int(3),
    arith_sub: "(- 10 3)" => Value::int(7),
    arith_mul: "(* 4 5)" => Value::int(20),
    arith_div: "(/ 10 2)" => Value::int(5),
    arith_mod: "(mod 10 3)" => Value::int(1),
    arith_mixed_float: "(+ 1 2.0)" => Value::float(3.0),
    arith_identity_add: "(+)" => Value::int(0),
    arith_identity_mul: "(*)" => Value::int(1),
    unary_minus: "(- 5)" => Value::int(-5),
    negative: "(+ -3 -7)" => Value::int(-10),
    mixed_int_float: "(* 2 3.5)" => Value::float(7.0),
    pow_basic: "(pow 2 10)" => Value::int(1024),
    sqrt_basic: "(sqrt 16)" => Value::int(4),
    abs_basic: "(abs -5)" => Value::int(5),
    min_basic: "(min 3 1 2)" => Value::int(1),
    max_basic: "(max 3 1 2)" => Value::int(3),
    // floor/ceiling/round/truncate are exactness-preserving (R7RS): a float
    // argument rounds to a float, not an int (see Task 5.2).
    floor_basic: "(floor 3.7)" => Value::float(3.0),
    ceil_basic: "(ceil 3.2)" => Value::float(4.0),
    round_basic: "(round 3.5)" => Value::float(4.0),
    pi_const: "(> pi 3.14)" => Value::bool(true),
    e_const: "(> e 2.71)" => Value::bool(true),
    math_clamp: "(math/clamp 15 0 10)" => Value::int(10),
    math_sign_pos: "(math/sign 42)" => Value::int(1),
    math_sign_neg: "(math/sign -3)" => Value::int(-1),
    math_sign_zero: "(math/sign 0)" => Value::int(0),
    trig_sin: "(> (sin 0.0) -0.001)" => Value::bool(true),
}

// ============================================================
// Comparison & Logic
// ============================================================

eval_tests! {
    cmp_lt: "(< 1 2)" => Value::bool(true),
    cmp_gt: "(> 3 2)" => Value::bool(true),
    cmp_lte: "(<= 2 2)" => Value::bool(true),
    cmp_eq: "(= 42 42)" => Value::bool(true),
    cmp_not: "(not #f)" => Value::bool(true),
    chained_cmp: "(< 1 2 3 4)" => Value::bool(true),
    chained_cmp_fail: "(< 1 2 2 3)" => Value::bool(false),
    truthiness_0: "(if 0 :truthy :falsy)" => Value::keyword("truthy"),
    truthiness_empty: r#"(if "" :truthy :falsy)"# => Value::keyword("truthy"),
    truthiness_nil: "(if nil :truthy :falsy)" => Value::keyword("falsy"),
    truthiness_false: "(if #f :truthy :falsy)" => Value::keyword("falsy"),
}

// ============================================================
// Core Forms (define, let, begin, set!, lambda, closures)
// ============================================================

eval_tests! {
    define_var: "(begin (define x 42) x)" => Value::int(42),
    define_fn: "(begin (define (square x) (* x x)) (square 5))" => Value::int(25),
    defun_alias: "(begin (defun square (x) (* x x)) (square 5))" => Value::int(25),
    fn_alias: "((fn (x) (* x x)) 4)" => Value::int(16),
    lambda_basic: "((lambda (x y) (+ x y)) 3 4)" => Value::int(7),
    lambda_multi_body: "((lambda (x) (define y 2) (* x y)) 5)" => Value::int(10),
    let_basic: "(let ((x 10) (y 20)) (+ x y))" => Value::int(30),
    let_empty: "(let () 42)" => Value::int(42),
    let_star: "(let* ((x 10) (y (* x 2))) (+ x y))" => Value::int(30),
    letrec_basic: "(letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1))))) (odd? (lambda (n) (if (= n 0) #f (even? (- n 1)))))) (even? 10))" => Value::bool(true),
    begin_basic: "(begin 1 2 3)" => Value::int(3),
    begin_empty: "(begin)" => Value::nil(),
    set_bang: "(begin (define x 1) (set! x 2) x)" => Value::int(2),
    closure_adder: "(begin (define (make-adder n) (lambda (x) (+ n x))) ((make-adder 5) 3))" => Value::int(8),
    closure_counter: "(begin (define (make-counter) (let ((n 0)) (lambda () (set! n (+ n 1)) n))) (define c (make-counter)) (c) (c) (c))" => Value::int(3),
    nested_closure: "(begin (define (f x) (lambda (y) (lambda (z) (+ x y z)))) (((f 1) 2) 3))" => Value::int(6),
    internal_define: "(begin (define (foo x) (define y 10) (+ x y)) (foo 5))" => Value::int(15),
    // Inner define forward references (letrec* semantics)
    inner_define_forward_ref: "(begin (define (outer) (define (a) (b)) (define (b) 42) (a)) (outer))" => Value::int(42),
    inner_define_mutual_recursion: "(begin
        (define (outer n)
          (define (even? x) (if (= x 0) #t (odd? (- x 1))))
          (define (odd? x) (if (= x 0) #f (even? (- x 1))))
          (even? n))
        (outer 10))" => Value::bool(true),
    inner_define_three_way_forward: "(begin
        (define (outer)
          (define (a) (b))
          (define (b) (c))
          (define (c) 99)
          (a))
        (outer))" => Value::int(99),
    inner_define_value_and_fn: "(begin
        (define (outer x)
          (define scale 10)
          (define (helper y) (* y scale))
          (helper x))
        (outer 5))" => Value::int(50),
    inner_define_fn_refs_later_value: "(begin
        (define (outer)
          (define (f) factor)
          (define factor 7)
          (f))
        (outer))" => Value::int(7),
    inner_define_nqueens_pattern: "(begin
        (define (solve n)
          (define (try-it x) (if (ok? x) x 0))
          (define (ok? x) (> x 0))
          (try-it n))
        (solve 5))" => Value::int(5),
    inner_define_with_closure_capture: "(begin
        (define (outer n)
          (define (inc x) (+ x step))
          (define step 3)
          (inc n))
        (outer 10))" => Value::int(13),
    inner_define_nested_bodies: "(begin
        (define (outer)
          (define (mid)
            (define (a) (b))
            (define (b) 42)
            (a))
          (mid))
        (outer))" => Value::int(42),
    inner_define_in_let_body: "(begin
        (define (outer)
          (let ((x 1))
            (define (a) (+ x (b)))
            (define (b) 42)
            (a)))
        (outer))" => Value::int(43),
    inner_define_in_let_star_body: "(begin
        (define (outer)
          (let* ((x 1) (y 2))
            (define (a) (+ x y (b)))
            (define (b) 10)
            (a)))
        (outer))" => Value::int(13),
    inner_define_in_letrec_body: "(begin
        (define (outer)
          (letrec ((f (lambda (n) (if (= n 0) (g) (f (- n 1))))))
            (define (g) 99)
            (f 3)))
        (outer))" => Value::int(99),
    rest_params: "(begin (define (sum . args) (foldl + 0 args)) (sum 1 2 3 4 5))" => Value::int(15),
    higher_order: "(begin (define (compose f g) (lambda (x) (f (g x)))) ((compose (lambda (x) (* x 2)) (lambda (x) (+ x 1))) 5))" => Value::int(12),
}

// ============================================================
// Control Flow (if, cond, case, when, unless, and, or, do)
// ============================================================

eval_tests! {
    if_true: "(if #t 1 2)" => Value::int(1),
    if_false: "(if #f 1 2)" => Value::int(2),
    if_two_branch: "(if (> 3 2) :yes :no)" => Value::keyword("yes"),
    cond_basic: "(cond ((= 1 2) 10) ((= 1 1) 20) (else 30))" => Value::int(20),
    cond_no_match: "(cond ((= 1 2) 10) ((= 1 3) 20))" => Value::nil(),
    case_basic: "(case 2 ((1) :one) ((2) :two) (else :other))" => Value::keyword("two"),
    case_else: "(case 99 ((1) :one) (else :other))" => Value::keyword("other"),
    and_all_true: "(and 1 2 3)" => Value::int(3),
    and_short_circuit: "(and 1 #f 3)" => Value::bool(false),
    and_empty: "(and)" => Value::bool(true),
    or_found: "(or #f #f 3)" => Value::int(3),
    or_first: "(or 1 2 3)" => Value::int(1),
    or_empty: "(or)" => Value::bool(false),
    when_true: "(when #t 42)" => Value::int(42),
    when_false: "(when #f 42)" => Value::nil(),
    unless_false: "(unless #f 42)" => Value::int(42),
    unless_true: "(unless #t 42)" => Value::nil(),
    do_loop_sum: "(do ((i 0 (+ i 1)) (sum 0 (+ sum i))) ((= i 5) sum))" => Value::int(10),
    do_loop_factorial: "(do ((n 5 (- n 1)) (acc 1 (* acc n))) ((= n 0) acc))" => Value::int(120),
    named_let_sum: "(let loop ((n 10) (acc 0)) (if (= n 0) acc (loop (- n 1) (+ acc n))))" => Value::int(55),
    named_let_fib: "(let fib ((n 10)) (cond ((= n 0) 0) ((= n 1) 1) (else (+ (fib (- n 1)) (fib (- n 2))))))" => Value::int(55),
}

// ============================================================
// Quote, Quasiquote, Eval
// ============================================================

eval_tests! {
    // Foundational ops: hand-constructed expected values so the oracle does not
    // depend on the tree-walker (see docs/bugs/eval-tw-oracle-circularity.md).
    quote_list: "(car '(a b c))" => Value::symbol("a"),
    quasiquote_basic: "(begin (define x 42) (car (cdr `(a ,x b))))" => Value::int(42),
    unquote_splicing: "(begin (define xs '(2 3)) `(1 ,@xs 4))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3), Value::int(4)]),
    eval_basic: "(eval '(+ 1 2))" => Value::int(3),
    read_parse: r#"(eval (read "(+ 10 20)"))"# => Value::int(30),
    macroexpand_basic: "(begin (defmacro my-if (c t e) (list 'if c t e)) (macroexpand '(my-if #t 1 2)))" => Value::list(vec![Value::symbol("if"), Value::bool(true), Value::int(1), Value::int(2)]),
    defmacro_basic: "(begin (defmacro my-if (c t e) (list 'if c t e)) (my-if #t 1 2))" => Value::int(1),
    defmacro_inside_map_literal: "(begin (defmacro one () 1) (:x {:x (one)}))" => Value::int(1),
    defmacro_inside_vector_literal: "(begin (defmacro one () 1) (nth [(one)] 0))" => Value::int(1),
    gensym_symbol: "(symbol? (gensym))" => Value::bool(true),
}

// ============================================================
// Error Handling (try/catch/throw)
// ============================================================

eval_tests! {
    try_no_error: "(try 42 (catch e 0))" => Value::int(42),
    try_catch_error: r#"(try (error "boom") (catch e 99))"# => Value::int(99),
    try_catch_division: "(try (/ 1 0) (catch e :caught))" => Value::keyword("caught"),
    throw_catch_value: r#"(try (throw "oops") (catch e (:value e)))"# => Value::string("oops"),
    nested_try: r#"(try (try (error "inner") (catch e (error "outer"))) (catch e2 :recovered))"# => Value::keyword("recovered"),
}

// ============================================================
// TCO
// ============================================================

eval_tests! {
    tco_deep: "(begin (define (count-down n) (if (= n 0) :done (count-down (- n 1)))) (count-down 100000))" => Value::keyword("done"),
    tco_mutual: "(begin (define (even? n) (if (= n 0) #t (odd? (- n 1)))) (define (odd? n) (if (= n 0) #f (even? (- n 1)))) (even? 1000))" => Value::bool(true),
    factorial_10: "(begin (define (factorial n) (if (<= n 1) 1 (* n (factorial (- n 1))))) (factorial 10))" => Value::int(3628800),
}

// ============================================================
// Scheme Aliases & Misc
// ============================================================

eval_tests! {
    cons_basic: "(cons 1 '(2 3))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
    car_cdr: "(car (cdr '(1 2 3)))" => Value::int(2),
    cadr: "(cadr '(1 2 3))" => Value::int(2),
    eq_identity: "(eq? 42 42)" => Value::bool(true),
    equal: "(equal? '(1 2) '(1 2))" => Value::bool(true),
    scheme_null: "(null? '())" => Value::bool(true),
    scheme_pair: "(pair? '(1 2))" => Value::bool(true),
}

// ============================================================
// Error tests
// ============================================================

eval_error_tests! {
    err_division_by_zero: "(/ 1 0)",
    err_unbound_var: "undefined-variable",
}

// ============================================================
// Case Edge Cases
// ============================================================

eval_tests! {
    case_multiple_datums: "(case 2 ((1 2 3) :found) (else :miss))" => Value::keyword("found"),
    case_no_match_no_else: "(case 99 ((1 2) :x))" => Value::nil(),
    case_multiple_body: "(begin (define x 0) (case 1 ((1) (set! x 7) x) (else 0)))" => Value::int(7),
    case_key_eval_once: "(begin (define n 0) (define (k) (set! n (+ n 1)) n) (case (k) ((1) n) (else 0)))" => Value::int(1),
}

// ============================================================
// Try/Catch/Throw Edge Cases
// ============================================================

eval_tests! {
    try_throw_map_catch_value: "(try (throw {:a 1}) (catch e (get e :value)))" => Value::map(BTreeMap::from([(Value::keyword("a"), Value::int(1))])),
    try_side_effect_before_throw: "(begin (define x 0) (try (set! x 1) (throw \"boom\") (catch e x)))" => Value::int(1),
    try_nested_rethrow: "(try (try (throw 42) (catch e (throw (+ 1 (get e :value))))) (catch e2 (get e2 :value)))" => Value::int(43),
    try_catch_multi_body: r#"(try (throw "err") (catch e (define y 10) (+ y 5)))"# => Value::int(15),
    try_no_error_multi_body: "(try 1 2 3 (catch e 0))" => Value::int(3),
}

// ============================================================
// Guard (R7RS structured exception handling)
// ============================================================

eval_tests! {
    // R7RS: the guard variable is bound to the RAISED OBJECT itself (not an
    // error-map wrapper). (raise obj) / (throw obj) both raise obj raw.
    guard_raise_binds_raw_object: r#"(guard (e (#t (list 'caught e))) (raise "oops"))"# => common::eval(r#"'(caught "oops")"#),
    guard_raise_int_raw: "(guard (e (#t e)) (raise 42))" => Value::int(42),
    guard_raise_symbol_raw: "(guard (e (#t e)) (raise 'x))" => Value::symbol("x"),
    guard_throw_alias_raw: "(guard (e (#t e)) (throw 7))" => Value::int(7),
    guard_predicate_clause_on_raw: r#"(guard (e ((string? e) e) (else :unknown)) (raise "x"))"# => Value::string("x"),
    guard_else_fallback: "(guard (e ((equal? e :bad) :handled) (else :fallback)) (raise :other))" => Value::keyword("fallback"),
    guard_body_no_condition: "(guard (e (#t (+ 1 e))) 100)" => Value::int(100),
    guard_multi_clause_dispatch: "(guard (e ((equal? e 5) 'a) ((equal? e 6) 'b) (else 'c)) (raise 6))" => Value::symbol("b"),
    guard_tail_position: "((lambda () (guard (e (else e)) (raise 99))))" => Value::int(99),
    // A native runtime error has no raw raised object, so the variable is the
    // error MAP; dispatch on (:type e)/(:message e).
    guard_native_error_type: "(guard (e (else (:type e))) (/ 1 0))" => Value::keyword("eval"),
    guard_native_error_message_is_string: "(guard (e (else (string? (:message e)))) (/ 1 0))" => Value::bool(true),
    // Genuine runtime error (car of a non-sequence is a type error) IS caught.
    guard_catches_runtime_type_error: "(guard (e (#t 'recovered)) (car 5))" => Value::symbol("recovered"),
    // Re-raise is faithful: an inner guard with no matching clause re-raises the
    // raw object; the outer guard again sees the same raw object.
    guard_nested_faithful_reraise: "(guard (e (#t e)) (guard (inner ((equal? inner 1) :x)) (raise 2)))" => Value::int(2),
    guard_else_can_reraise_fresh: "(guard (e (#t e)) (guard (inner (else (raise 500))) (raise 400)))" => Value::int(500),
    // `raise` is a first-class procedure; try/catch sees the {:value ...} wrapper.
    guard_raise_procedure_via_try: "(try (raise 5) (catch e (:value e)))" => Value::int(5),
}

eval_error_tests! {
    // No clause matches and there is no `else`: the condition is re-raised.
    guard_no_match_reraises: "(guard (e ((equal? e 1) :one)) (raise 2))" => "User exception: 2",
    guard_empty_clauses_reraise: "(guard (e) (raise 7))" => "User exception: 7",
    // A native error whose clause doesn't match is re-raised; its message survives.
    guard_native_no_match_reraises: "(guard (e ((equal? e 1) :one)) (/ 1 0))" => "division by zero",
    // (error "oops") has :type :eval, not :user, so this user-only clause fails
    // and, with no else, the condition re-raises (message preserved).
    guard_error_type_dispatch_reraise: r#"(guard (e ((and (map? e) (eq? (:type e) :user)) (:value e))) (error "oops"))"# => "oops",
}

// ============================================================
// Do Loop Edge Cases
// ============================================================

eval_tests! {
    do_immediate_term_with_result: "(do ((i 0 (+ i 1))) ((= i 0) i))" => Value::int(0),
    do_immediate_term_no_result: "(do ((i 0 (+ i 1))) ((= i 0)))" => Value::nil(),
    do_parallel_step: "(do ((a 1 b) (b 2 a) (n 0 (+ n 1))) ((= n 1) (list a b)))" => Value::list(vec![Value::int(2), Value::int(1)]),
    do_step_side_effects: "(begin (define t 0) (do ((i 0 (begin (set! t (+ t 1)) (+ i 1)))) ((= i 3) t)))" => Value::int(3),
    do_body_side_effects: "(begin (define acc '()) (do ((i 0 (+ i 1))) ((= i 3) (reverse acc)) (set! acc (cons i acc))))" => Value::list(vec![Value::int(0), Value::int(1), Value::int(2)]),
}

// ============================================================
// Cond Edge Cases
// ============================================================

eval_tests! {
    cond_multi_body: "(cond ((= 1 1) 10 20 30))" => Value::int(30),
    cond_else_multi_body: "(cond (#f 1) (else 10 20))" => Value::int(20),
}

// ============================================================
// When/Unless Multi-Body
// ============================================================

eval_tests! {
    when_multi_body: "(when #t 1 2 3)" => Value::int(3),
    unless_multi_body: "(unless #f 1 2 3)" => Value::int(3),
}

// ============================================================
// And/Or Edge Cases
// ============================================================

eval_tests! {
    and_single_false: "(and #f)" => Value::bool(false),
    and_single_true: "(and 42)" => Value::int(42),
    and_returns_nil: "(and 1 nil 3)" => Value::nil(),
    and_returns_false: "(and 1 #f 3)" => Value::bool(false),
    and_nil_first: "(and nil 1)" => Value::nil(),
    or_all_false: "(or #f #f #f)" => Value::bool(false),
    or_with_nil: "(or nil nil 3)" => Value::int(3),
    or_single: "(or 42)" => Value::int(42),
}

// ============================================================
// While
// ============================================================

eval_tests! {
    while_basic: "(begin (define i 0) (while (< i 5) (set! i (+ i 1))) i)" => Value::int(5),
    while_returns_nil: "(begin (define i 0) (while (< i 3) (set! i (+ i 1))))" => Value::nil(),
    while_no_iterations: "(begin (define i 10) (while (< i 0) (set! i (+ i 1))) i)" => Value::int(10),
    while_multi_body: "(begin (define a 0) (define b 0) (while (< a 3) (set! a (+ a 1)) (set! b (+ b 10))) b)" => Value::int(30),
    while_nested: "(begin (define total 0) (define i 0) (while (< i 3) (define j 0) (while (< j 3) (set! total (+ total 1)) (set! j (+ j 1))) (set! i (+ i 1))) total)" => Value::int(9),
}

eval_error_tests! {
    while_bad_arity: "(while #t)",
}

// ============================================================
// Open Upvalue Close Semantics
// ============================================================

eval_tests! {
    upvalue_close_on_return: "(begin
    (define (make-getter)
      (define n 42)
      (lambda () n))
    ((make-getter)))" => Value::int(42),
    upvalue_shared_cell: "(begin
    (define (make-shared)
      (define n 0)
      (define inc (lambda () (set! n (+ n 1))))
      (define get (lambda () n))
      (list inc get))
    (define p (make-shared))
    ((first p))
    ((first p))
    ((car (cdr p))))" => Value::int(2),
    upvalue_late_mutation: "(begin
    (define (make-late)
      (define n 0)
      (define f (lambda () n))
      (set! n 42)
      f)
    ((make-late)))" => Value::int(42),
    upvalue_multi_level_close: "(begin
    (define (outer)
      (define x 1)
      (define (middle)
        (lambda () x))
      (set! x 99)
      (middle))
    ((outer)))" => Value::int(99),
    upvalue_tail_call_closes: "(begin
    (define captured #f)
    (define (setup)
      (define x 10)
      (set! captured (lambda () x))
      (set! x 20)
      :done)
    (setup)
    (captured))" => Value::int(20),
    upvalue_exception_closes: r#"(begin
    (define escaped #f)
    (try
      (begin
        (define x 0)
        (set! escaped (lambda () x))
        (set! x 99)
        (throw "boom"))
      (catch e (escaped))))"# => Value::int(99),
    upvalue_closure_via_hof: "(begin
    (define (test)
      (define n 42)
      (map (lambda (x) n) (list 1 2 3)))
    (test))" => Value::list(vec![Value::int(42), Value::int(42), Value::int(42)]),
    upvalue_mutable_via_hof: "(begin
    (define (test)
      (define n 0)
      (define inc (lambda (x) (set! n (+ n 1)) n))
      (map inc (list 1 2 3)))
    (test))" => Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),

    // ==========================================================
    // Shadowed builtins: optimizer must not constant-fold when
    // arithmetic operators are rebound by local bindings
    // ==========================================================

    // Basic shadowing via let for each foldable operator
    shadow_plus: "(let ((+ *)) (+ 3 4))" => Value::int(12),
    shadow_minus: "(let ((- +)) (- 3 4))" => Value::int(7),
    shadow_mul: "(let ((* +)) (* 3 4))" => Value::int(7),
    shadow_div: "(let ((/ -)) (/ 10 3))" => Value::int(7),
    shadow_lt: "(let ((< >)) (< 1 2))" => Value::bool(false),
    shadow_gt: "(let ((> <)) (> 5 3))" => Value::bool(false),
    shadow_le: "(let ((<= >=)) (<= 1 2))" => Value::bool(false),
    shadow_ge: "(let ((>= <=)) (>= 5 3))" => Value::bool(false),
    shadow_eq: r#"(let ((= (lambda (a b) (string-append (number->string a) (number->string b))))) (= 1 2))"# => Value::string("12"),
    shadow_not: "(let ((not (lambda (x) 42))) (not #f))" => Value::int(42),
    shadow_unary_minus: "(let ((- (lambda (x) (* x 2)))) (- 5))" => Value::int(10),

    // Shadowing via let* (sequential bindings)
    shadow_let_star: "(let* ((+ *) (x (+ 3 4))) x)" => Value::int(12),

    // Shadowing via letrec
    shadow_letrec: "(letrec ((+ (lambda (a b) (* a b)))) (+ 3 4))" => Value::int(12),

    // Shadowing via lambda parameter
    shadow_lambda_param: "((lambda (+ -) (+ 10 3)) * /)" => Value::int(30),
    shadow_lambda_param_unary: "((lambda (-) (- 5)) (lambda (x) (* x 10)))" => Value::int(50),

    // Shadowing via define inside begin
    shadow_define: "(begin (define + *) (+ 3 4))" => Value::int(12),

    // Nested scopes: inner shadow, outer unshadowed
    shadow_nested_inner: "(+ 3 (let ((+ *)) (+ 2 5)) )" => Value::int(13),
    shadow_nested_outer_ok: "(let ((x (+ 3 4))) (let ((+ *)) (+ x 2)))" => Value::int(14),

    // Shadow only in scope — unshadowed after let exits
    shadow_scope_exit: "(begin (define r1 (let ((+ *)) (+ 3 4))) (define r2 (+ 3 4)) (list r1 r2))" => Value::list(vec![Value::int(12), Value::int(7)]),

    // Deeply nested shadow
    shadow_deep: "(let ((+ *)) (let ((y 2)) (let ((z 3)) (+ y z))))" => Value::int(6),

    // Shadow with non-constant args (optimizer shouldn't fold anyway, but make sure
    // the shadowed version is called at runtime)
    shadow_non_const: "(let ((+ *)) (define a 3) (define b 4) (+ a b))" => Value::int(12),

    // Shadow in do loop step expression
    shadow_do_step: "(let ((+ *))
      (do ((i 1 (+ i 2)))
          ((> i 10) i)))" => Value::int(16),

    // Shadow comparison in cond test
    shadow_cond: "(let ((< >)) (cond ((< 1 2) :wrong) (#t :right)))" => Value::keyword("right"),

    // Shadow with higher-order: pass shadowed op as argument
    shadow_higher_order: "(let ((+ *)) (map (lambda (x) (+ x 2)) '(3 4 5)))" => Value::list(vec![Value::int(6), Value::int(8), Value::int(10)]),

    // Ensure non-shadowed still folds correctly (regression guard)
    no_shadow_still_folds: "(+ 100 200)" => Value::int(300),
    no_shadow_still_folds_nested: "(let ((x 1)) (+ 100 200))" => Value::int(300),
    no_shadow_unrelated_binding: "(let ((y 99)) (+ 3 4))" => Value::int(7),
}
