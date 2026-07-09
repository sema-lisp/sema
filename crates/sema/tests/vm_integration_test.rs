use sema_core::Value;
use sema_eval::Interpreter;

/// Evaluate via bytecode VM
fn eval_vm(input: &str) -> Value {
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(input)
        .unwrap_or_else(|_| panic!("VM failed: {input}"))
}

/// Evaluate `input` on the VM and assert it succeeds. (This is a single-evaluator
/// smoke check that the construct evaluates without error — correctness for the
/// canonical cases is pinned with a literal via [`assert_evals_to`].)
fn assert_evals(input: &str) {
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(input)
        .unwrap_or_else(|e| panic!("eval failed for `{input}`: {e}"));
}

/// Evaluate `input` on the VM and assert it equals `expected` (a literal oracle —
/// catches a regression that a bare smoke check would miss).
fn assert_evals_to(input: &str, expected: Value) {
    let interp = Interpreter::new();
    let result = interp
        .eval_str_compiled(input)
        .unwrap_or_else(|e| panic!("eval failed for `{input}`: {e}"));
    assert_eq!(result, expected, "unexpected result for `{input}`");
}

// === VM evaluation tests (smoke + literal oracle) ===

#[test]
fn test_vm_arithmetic() {
    // Canonical arithmetic cases are pinned to literal expected values so a
    // regression that breaks evaluation is still caught here.
    assert_evals_to("(+ 1 2)", Value::int(3));
    assert_evals_to("(- 10 3)", Value::int(7));
    assert_evals_to("(* 4 5)", Value::int(20));
    assert_evals_to("(/ 10 2)", Value::int(5));
    assert_evals_to("(+ 1 2.0)", Value::float(3.0));
    assert_evals_to("(+ 1 2 3 4 5)", Value::int(15));
    assert_evals_to("(* 2 3 4)", Value::int(24));
    assert_evals_to("(- 100 1 2 3)", Value::int(94));
}

#[test]
fn test_vm_comparison() {
    // Canonical predicate/comparison cases pinned to literal expected values.
    assert_evals_to("(< 1 2)", Value::bool(true));
    assert_evals_to("(> 3 2)", Value::bool(true));
    assert_evals_to("(= 42 42)", Value::bool(true));
    assert_evals_to("(not #f)", Value::bool(true));
    assert_evals_to("(< 2 1)", Value::bool(false));
    assert_evals_to("(= 1 2)", Value::bool(false));
    assert_evals_to("(not #t)", Value::bool(false));
    assert_evals_to("(<= 1 1)", Value::bool(true));
    assert_evals_to("(>= 2 3)", Value::bool(false));
}

#[test]
fn test_vm_define_and_call() {
    assert_evals("(begin (define x 42) x)");
    assert_evals("(begin (define (square x) (* x x)) (square 5))");
}

#[test]
fn test_vm_if() {
    assert_evals("(if #t 1 2)");
    assert_evals("(if #f 1 2)");
    assert_evals("(if (= 1 1) \"yes\" \"no\")");
}

#[test]
fn test_vm_let_forms() {
    assert_evals("(let ((x 10)) x)");
    assert_evals("(let ((x 1) (y 2)) (+ x y))");
    assert_evals("(let* ((x 1) (y (+ x 1))) y)");
}

#[test]
fn test_vm_letrec() {
    assert_evals("(letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1))))) (odd? (lambda (n) (if (= n 0) #f (even? (- n 1)))))) (even? 10))");
}

#[test]
fn test_vm_closures() {
    assert_evals("(let ((x 10)) ((lambda () x)))");
    assert_evals("(let ((n 0)) (let ((inc (lambda () (set! n (+ n 1)) n))) (inc) (inc) (inc)))");
}

#[test]
fn test_vm_recursion() {
    assert_evals("(begin (define (fact n) (if (= n 0) 1 (* n (fact (- n 1))))) (fact 10))");
}

#[test]
fn test_vm_named_let() {
    assert_evals("(let loop ((n 5) (acc 1)) (if (= n 0) acc (loop (- n 1) (* acc n))))");
}

#[test]
fn test_vm_do_loop() {
    assert_evals("(do ((i 0 (+ i 1))) ((= i 5) i))");
    assert_evals("(do ((i 0 (+ i 1)) (sum 0 (+ sum i))) ((= i 10) sum))");
}

#[test]
fn test_vm_try_catch_no_error() {
    assert_evals("(try 42 (catch e 99))");
}

#[test]
fn test_vm_try_catch_runtime_error() {
    assert_evals("(try (/ 1 0) (catch e \"caught\"))");
}

#[test]
fn test_vm_try_catch_thrown_value() {
    assert_evals("(try (throw \"boom\") (catch e e))");
}

#[test]
fn test_vm_and_or() {
    assert_evals("(and)");
    assert_evals("(or)");
    assert_evals("(and #t 42)");
    assert_evals("(and #f 42)");
    assert_evals("(or 42 99)");
    assert_evals("(or #f 99)");
}

#[test]
fn test_vm_data_constructors() {
    assert_evals("(list 1 2 3)");
    assert_evals("[1 2 3]");
    assert_evals("'(a b c)");
}

#[test]
fn test_vm_rest_params() {
    assert_evals("((lambda (x . rest) rest) 1 2 3)");
    assert_evals("((lambda (x . rest) x) 1 2 3)");
}

#[test]
fn test_vm_begin() {
    assert_evals("(begin 1 2 3)");
    assert_evals("(begin)");
}

#[test]
fn test_vm_set() {
    assert_evals("(begin (define x 1) (set! x 42) x)");
}

#[test]
fn test_vm_string_ops() {
    assert_evals("(string-length \"hello\")");
    assert_evals("(string-append \"a\" \"b\" \"c\")");
    assert_evals("(substring \"hello\" 1 3)");
}

#[test]
fn test_vm_list_ops() {
    assert_evals("(car '(1 2 3))");
    assert_evals("(cdr '(1 2 3))");
    assert_evals("(cons 1 '(2 3))");
    assert_evals("(length '(1 2 3))");
    assert_evals("(null? '())");
    assert_evals("(null? '(1))");
}

#[test]
fn test_vm_higher_order() {
    assert_evals("(map (lambda (x) (* x x)) '(1 2 3 4))");
    assert_evals("(filter (lambda (x) (> x 2)) '(1 2 3 4 5))");
    assert_evals("(foldl + 0 '(1 2 3 4 5))");
}

#[test]
fn test_vm_keyword_as_function() {
    assert_evals("(:a {:a 1 :b 2})");
    assert_evals("(:missing {:a 1})");
}

#[test]
fn test_vm_when_unless() {
    assert_evals("(when #t 42)");
    assert_evals("(when #f 42)");
    assert_evals("(unless #t 42)");
    assert_evals("(unless #f 42)");
}

#[test]
fn test_vm_cond() {
    assert_evals("(cond (#f 1) (#t 2) (else 3))");
    assert_evals("(cond (#f 1) (#f 2) (else 3))");
}

#[test]
fn test_vm_case() {
    assert_evals("(case 2 ((1) \"one\") ((2) \"two\") (else \"other\"))");
}

// === Targeted delegate tests ===

#[test]
fn test_vm_delay_is_lazy() {
    // delay should NOT evaluate its body until forced
    let interp = Interpreter::new();
    interp.eval_str_compiled("(define x 0)").unwrap();
    interp
        .eval_str_compiled("(define t (delay (begin (set! x (+ x 1)) x)))")
        .unwrap();
    let result = interp.eval_str_compiled("x").unwrap();
    assert_eq!(
        result,
        Value::int(0),
        "delay body should not be evaluated eagerly"
    );
}

#[test]
fn test_vm_force_evaluates_thunk() {
    let interp = Interpreter::new();
    interp.eval_str_compiled("(define x 0)").unwrap();
    interp
        .eval_str_compiled("(define t (delay (begin (set! x (+ x 1)) x)))")
        .unwrap();
    let result = interp.eval_str_compiled("(force t)").unwrap();
    assert_eq!(result, Value::int(1));
    // force again should return memoized value
    let result2 = interp.eval_str_compiled("(force t)").unwrap();
    assert_eq!(result2, Value::int(1));
    // x should still be 1 (not 2)
    let x = interp.eval_str_compiled("x").unwrap();
    assert_eq!(x, Value::int(1), "force should memoize");
}

#[test]
fn test_vm_define_record_type() {
    let result = eval_vm("(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point-x (make-point 3 4)))");
    assert_eq!(result, Value::int(3));
    let result2 = eval_vm("(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point-y (make-point 3 4)))");
    assert_eq!(result2, Value::int(4));
    let result3 = eval_vm("(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point? (make-point 1 2)))");
    assert_eq!(result3, Value::bool(true));
}

#[test]
fn test_vm_define_record_type_persists() {
    let interp = Interpreter::new();
    interp
        .eval_str_compiled(
            "(define-record-type point (make-point x y) point? (x point-x) (y point-y))",
        )
        .unwrap();
    let result = interp
        .eval_str_compiled("(point-x (make-point 3 4))")
        .unwrap();
    assert_eq!(result, Value::int(3));
}

#[test]
fn test_vm_eval_delegate() {
    let result = eval_vm("(eval '(+ 1 2))");
    assert_eq!(result, Value::int(3));
}

#[test]
fn test_vm_apply() {
    assert_evals("(apply + '(1 2 3))");
    assert_evals("(apply list 1 2 '(3 4))");
    assert_evals("(apply list '(1 2 3))");
}

#[test]
fn test_vm_multiple_defines_persist() {
    // Verify that defines in compiled mode persist across eval calls
    let interp = Interpreter::new();
    interp.eval_str_compiled("(define a 1)").unwrap();
    interp.eval_str_compiled("(define b 2)").unwrap();
    let result = interp.eval_str_compiled("(+ a b)").unwrap();
    assert_eq!(result, Value::int(3));
}

#[test]
fn test_vm_nested_closures_shared_state() {
    // Two closures sharing a mutable upvalue
    let result = eval_vm(
        "(let ((n 0))
           (let ((inc (lambda () (set! n (+ n 1))))
                 (get (lambda () n)))
             (inc) (inc) (inc)
             (get)))",
    );
    assert_eq!(result, Value::int(3));
}

#[test]
fn test_vm_recursion_moderate_depth() {
    // Verify recursion works at moderate depth
    // Note: VM creates a fresh VM per closure call, so deep recursion
    // consumes native stack. ~200 is safe; true TCO would allow 100k+.
    let interp = Interpreter::new();
    interp
        .eval_str_compiled("(define (loop n) (if (= n 0) 0 (loop (- n 1))))")
        .unwrap();
    let result = interp.eval_str_compiled("(loop 50)").unwrap();
    assert_eq!(result, Value::int(0));
}

#[test]
fn test_vm_map_literal() {
    assert_evals("{:a 1 :b 2}");
}

#[test]
fn test_vm_threading_macros() {
    assert_evals(
        r#"(begin
          (defmacro -> (val . forms)
            (if (null? forms) val
              (let ((form (car forms)) (rest (cdr forms)))
                (if (list? form)
                  `(-> (,(car form) ,val ,@(cdr form)) ,@rest)
                  `(-> (,form ,val) ,@rest)))))
          (-> 5 (+ 3) (* 2)))"#,
    );
    assert_evals(
        r#"(begin
          (defmacro ->> (val . forms)
            (if (null? forms) val
              (let ((form (car forms)) (rest (cdr forms)))
                (if (list? form)
                  `(->> (,(car form) ,@(cdr form) ,val) ,@rest)
                  `(->> (,form ,val) ,@rest)))))
          (->> (list 1 2 3 4 5) (filter odd?) (map (lambda (x) (* x x)))))"#,
    );
}

// === Higher-order functions with closures ===

#[test]
fn test_vm_closure_returns_closure() {
    assert_evals("(begin (define (make-adder n) (lambda (x) (+ n x))) ((make-adder 5) 3))");
}

#[test]
fn test_vm_compose() {
    assert_evals("(begin (define (compose f g) (lambda (x) (f (g x)))) (define inc (lambda (x) (+ x 1))) (define dbl (lambda (x) (* x 2))) ((compose dbl inc) 5))");
}

#[test]
fn test_vm_curry() {
    assert_evals("(begin (define (curry f) (lambda (x) (lambda (y) (f x y)))) (((curry +) 3) 4))");
}

#[test]
fn test_vm_partial_application() {
    assert_evals(
        r#"(begin
        (define (partial f . args)
            (lambda (. rest) (apply f (append args rest))))
        (define add5 (partial + 5))
        (add5 3))"#,
    );
}

#[test]
fn test_vm_map_with_composed_fn() {
    assert_evals("(begin (define (compose f g) (lambda (x) (f (g x)))) (map (compose (lambda (x) (* x 2)) (lambda (x) (+ x 1))) (list 1 2 3)))");
}

#[test]
fn test_vm_filter_with_closure() {
    assert_evals("(begin (define (gt n) (lambda (x) (> x n))) (filter (gt 3) (list 1 2 3 4 5)))");
}

#[test]
fn test_vm_foldl_with_closure() {
    assert_evals("(foldl (lambda (acc x) (+ acc x)) 0 (list 1 2 3 4 5))");
}

#[test]
fn test_vm_nested_closures_3_levels() {
    // 3 nested lambdas, fully called — should return 42
    assert_evals("((((lambda () (lambda () (lambda () 42))))))");
    // 2 calls leaves a function — verify calling it produces 42
    assert_evals("((((lambda () (lambda () (lambda () 42))))))");
}

// === Map/HashMap operations ===

#[test]
fn test_vm_map_assoc() {
    assert_evals("(get (assoc {:a 1} :b 2) :b)");
}

#[test]
fn test_vm_map_merge() {
    assert_evals("(merge {:a 1} {:b 2})");
}

#[test]
fn test_vm_map_contains() {
    assert_evals("(contains? {:a 1 :b 2} :a)");
}

#[test]
fn test_vm_map_dissoc() {
    assert_evals("(dissoc {:a 1 :b 2 :c 3} :b)");
}

#[test]
fn test_vm_map_values() {
    assert_evals("(vals {:a 1 :b 2})");
}

#[test]
fn test_vm_hashmap_ops() {
    assert_evals("(begin (define h (hashmap/new :a 1 :b 2)) (get h :a))");
}

// === String operations ===

#[test]
fn test_vm_string_ref() {
    assert_evals("(string-ref \"hello\" 0)");
}

#[test]
fn test_vm_char_operations() {
    assert_evals("(char->integer #\\a)");
    assert_evals("(integer->char 65)");
}

#[test]
fn test_vm_string_map() {
    assert_evals(r#"(string/map char-upcase "hello")"#);
}

// === Comprehension-style patterns ===

#[test]
fn test_vm_flat_map() {
    assert_evals("(flat-map (lambda (x) (map (lambda (y) (list x y)) (list 1 2))) (list :a :b))");
}

#[test]
fn test_vm_nested_map() {
    assert_evals("(map (lambda (x) (map (lambda (y) (* x y)) (list 1 2 3))) (list 10 20))");
}

// === do loop patterns ===

#[test]
fn test_vm_do_with_body() {
    assert_evals("(let ((result '())) (do ((i 0 (+ i 1))) ((= i 5) (reverse result)) (set! result (cons i result))))");
}

#[test]
fn test_vm_do_fizzbuzz() {
    assert_evals(
        r#"(do ((i 1 (+ i 1)) (out '())) ((> i 15) (reverse out))
        (set! out (cons (cond ((= (mod i 15) 0) "FizzBuzz") ((= (mod i 3) 0) "Fizz") ((= (mod i 5) 0) "Buzz") (else i)) out)))"#,
    );
}

// === Defmacro patterns ===

#[test]
fn test_vm_defmacro_simple() {
    assert_evals("(begin (defmacro my-when (test body) (list 'if test body nil)) (my-when #t 42))");
}

#[test]
fn test_vm_defmacro_called_twice() {
    assert_evals("(begin (defmacro double (x) (list '+ x x)) (list (double 3) (double 5)))");
}

// === Multi-expression defines (data-pipeline patterns) ===

#[test]
fn test_vm_chained_map_filter() {
    assert_evals("(filter (lambda (x) (> x 5)) (map (lambda (x) (* x 2)) (list 1 2 3 4 5)))");
}

#[test]
fn test_vm_reduce_map_values() {
    assert_evals("(foldl + 0 (map (lambda (x) (get x :val)) (list {:val 1} {:val 2} {:val 3})))");
}

// === for-each ===

#[test]
fn test_vm_for_each() {
    assert_evals("(begin (define result '()) (for-each (lambda (x) (set! result (cons x result))) (list 1 2 3)) (reverse result))");
}

// === Recursive patterns ===

#[test]
fn test_vm_assoc_list() {
    assert_evals("(assq 'Bob '((Alice \"555-1234\") (Bob \"555-5678\")))");
}

#[test]
fn test_vm_sort() {
    assert_evals("(sort (list 3 1 4 1 5 9))");
}

// ============================================================================
// BUG ISOLATION TESTS
// These tests isolate specific root causes of VM failures found in examples.
// Each test targets a single root cause category.
// ============================================================================

// --- Bug 1: Self-ref injection corrupts locals for named functions ---
// vm.rs make_closure: func.name.is_some() causes slot `arity` to be overwritten
// with the NativeFn self-reference, clobbering normal locals.

#[test]
fn test_bug_named_let_inside_define() {
    // named-let loop inside a defined function
    // The loop fn has name=Some, arity=1, and the self-ref overwrites slot 1
    // which may be used by locals in the loop body
    assert_evals(
        "(begin
           (define (test-fn n)
             (let loop ((i 0))
               (if (= i n) i (loop (+ i 1)))))
           (test-fn 5))",
    );
}

#[test]
fn test_bug_named_let_inside_lambda() {
    // Same bug when the named-let is inside a lambda passed to a HOF
    assert_evals(
        "(map (lambda (n)
                (let loop ((i 0))
                  (if (= i n) i (loop (+ i 1)))))
              (list 1 2 3))",
    );
}

#[test]
fn test_bug_named_let_two_params_inside_define() {
    // Two-param named-let inside define, loop name at slot 2
    assert_evals(
        "(begin
           (define (repeat-string s n)
             (let loop ((i 0) (acc \"\"))
               (if (= i n) acc
                 (loop (+ i 1) (string-append acc s)))))
           (repeat-string \"ab\" 3))",
    );
}

#[test]
fn test_bug_named_let_captures_outer_variable() {
    // named-let loop that captures a variable from the enclosing define
    // compile_named_let emits 0 upvalues, so n is unresolvable
    assert_evals(
        "(begin
           (define (is-prime? n)
             (let loop ((i 2))
               (cond ((> (* i i) n) #t)
                     ((= 0 (mod n i)) #f)
                     (else (loop (+ i 1))))))
           (is-prime? 7))",
    );
}

#[test]
fn test_bug_named_let_nested_in_hof() {
    // named-let inside a lambda passed to any/every — the exact pattern
    // that fails in comprehensions.sema, scheme-basics.sema, etc.
    assert_evals(
        "(any (lambda (n)
                (let loop ((i 2))
                  (cond ((> (* i i) n) #t)
                        ((= 0 (mod n i)) #f)
                        (else (loop (+ i 1))))))
              (list 4 6 7 8 9))",
    );
}

// --- Bug 2: Missing arity checking in NativeFn closure wrapper ---
// vm.rs make_closure: args shorter than arity filled with Nil silently

#[test]
fn test_bug_arity_too_few_args() {
    // Calling a 2-arg VM closure with 1 arg should error, not silently use Nil
    let interp = Interpreter::new();
    let result = interp.eval_str_compiled("(begin (define (add a b) (+ a b)) (add 1))");
    assert!(
        result.is_err(),
        "calling 2-arg fn with 1 arg should be an arity error"
    );
}

#[test]
fn test_bug_arity_too_many_args() {
    // Calling a 1-arg fn with 3 args should error
    let interp = Interpreter::new();
    let result = interp.eval_str_compiled("(begin (define (id x) x) (id 1 2 3))");
    assert!(
        result.is_err(),
        "calling 1-arg fn with 3 args should be an arity error"
    );
}

// --- Bug 3: compile_named_let missing func_id patch ---
// compiler.rs compile_named_let: doesn't call patch_closure_func_ids
// on inner chunk when child functions exist

#[test]
fn test_bug_named_let_with_nested_lambda() {
    // named-let loop whose body creates a lambda — the inner lambda gets wrong func_id
    assert_evals(
        "(begin
           (define (build-list n)
             (let loop ((i 0) (acc '()))
               (if (= i n) (reverse acc)
                 (loop (+ i 1) (cons (+ i 1) acc)))))
           (build-list 5))",
    );
}

#[test]
fn test_bug_named_let_body_uses_map() {
    // named-let body calls map with a lambda — nested lambda inside named-let
    assert_evals(
        "(begin
           (define (process items)
             (let loop ((xs items) (acc '()))
               (if (null? xs) (reverse acc)
                 (loop (cdr xs) (cons (* (car xs) 2) acc)))))
           (process (list 1 2 3)))",
    );
}

// --- Bug 4: Fresh VM per closure causes stack overflow ---
// Each NativeFn closure creates VM::new, consuming native stack

#[test]
fn test_bug_moderate_recursion_via_define() {
    // 50 recursive calls via a defined function — should not overflow
    assert_evals(
        "(begin
           (define (countdown n) (if (= n 0) 0 (countdown (- n 1))))
           (countdown 50))",
    );
}

// --- Combined patterns from failing examples ---

#[test]
fn test_example_pattern_l_system() {
    // l-system.sema: string rewriting with named-let loop
    assert_evals(
        r#"(begin
           (define (rewrite s rules)
             (let loop ((i 0) (acc ""))
               (if (= i (string-length s)) acc
                 (let ((ch (string-ref s i)))
                   (loop (+ i 1)
                         (string-append acc
                           (let check ((rs rules))
                             (cond ((null? rs) (str ch))
                                   ((= (car (car rs)) ch) (cadr (car rs)))
                                   (else (check (cdr rs)))))))))))
           (rewrite "AB" (list (list #\A "AB") (list #\B "A"))))"#,
    );
}

#[test]
fn test_example_pattern_brainfuck_cell_ops() {
    // brainfuck.sema pattern: map-based state with assoc/get
    assert_evals(
        "(begin
           (define mem {:ptr 0 :cells (list 0 0 0)})
           (get mem :ptr))",
    );
}

#[test]
fn test_example_pattern_lazy_streams() {
    // lazy.sema: cons-based streams with delay/force
    assert_evals(
        "(begin
           (define (stream-cons head tail-thunk) (list head tail-thunk))
           (define (stream-car s) (car s))
           (define (stream-cdr s) (force (cadr s)))
           (define ones (stream-cons 1 (delay ones)))
           (stream-car ones))",
    );
}

#[test]
fn test_example_pattern_string_iteration() {
    // Pattern from multiple examples: iterating over string characters
    assert_evals(
        r#"(begin
           (define (count-chars s ch)
             (let loop ((i 0) (count 0))
               (if (= i (string-length s)) count
                 (loop (+ i 1) (if (= (string-ref s i) ch) (+ count 1) count)))))
           (count-chars "hello" #\l))"#,
    );
}

#[test]
fn test_example_pattern_recursive_tree_walk() {
    // Pattern from huffman-coding, meta-eval: recursive tree walking
    assert_evals(
        "(begin
           (define (tree-sum tree)
             (if (number? tree) tree
               (+ (tree-sum (car tree)) (tree-sum (cadr tree)))))
           (tree-sum (list (list 1 2) (list 3 4))))",
    );
}

#[test]
fn test_example_pattern_math_fold() {
    // math-and-crypto.sema: fold over range with closures
    assert_evals(
        "(begin
           (define (factorial n)
             (foldl * 1 (range 1 (+ n 1))))
           (factorial 5))",
    );
}

#[test]
fn test_example_pattern_defmacro_with_named_let() {
    // Pattern used in comprehensions.sema and others:
    // defmacro that generates code using named-let
    assert_evals(
        "(begin
           (defmacro repeat-n (n body)
             `(let loop ((i 0) (acc '()))
                (if (= i ,n) (reverse acc)
                  (loop (+ i 1) (cons ,body acc)))))
           (repeat-n 3 42))",
    );
}

#[test]
fn test_example_pattern_modules_import() {
    // Sema modules are file-path-based (Decision #19)
    assert_evals(
        "(begin
           (file/write \"/tmp/sema-vm-test-mod.sema\"
             \"(module math (export square) (define (square x) (* x x)))\")
           (import \"/tmp/sema-vm-test-mod.sema\")
           (square 5))",
    );
}

#[test]
fn test_example_pattern_modules_selective_import() {
    // Selective import: (import "path" sym1 sym2) with bare symbols
    assert_evals(
        "(begin
           (file/write \"/tmp/sema-vm-test-sel.sema\"
             \"(module sel (export square cube) (define (square x) (* x x)) (define (cube x) (* x x x)))\")
           (import \"/tmp/sema-vm-test-sel.sema\" square)
           (square 5))",
    );
}

// ============================================================================
// BUG 4: STACK OVERFLOW — fresh VM per closure exhausts native stack
// ============================================================================

#[test]
fn test_bug_recursion_depth_50() {
    // 50 recursive calls — works on test thread's smaller stack
    assert_evals(
        "(begin
           (define (count n) (if (= n 0) 0 (count (- n 1))))
           (count 50))",
    );
}

#[test]
fn test_bug_recursion_depth_1000() {
    // VM should handle 1000
    // TODO: Stress test this to 10 million to see where it breaks
    assert_evals(
        "(begin
           (define (count n) (if (= n 0) 0 (count (- n 1))))
           (count 1000))",
    );
}

#[test]
fn test_bug_tail_recursion_depth_100000() {
    // Named-let loop with TCO — should handle any depth
    assert_evals(
        "(let loop ((n 100000) (acc 0))
           (if (= n 0) acc (loop (- n 1) (+ acc n))))",
    );
}

#[test]
fn test_bug_mutual_recursion_depth() {
    assert_evals(
        "(begin
           (define (even? n) (if (= n 0) #t (odd? (- n 1))))
           (define (odd? n) (if (= n 0) #f (even? (- n 1))))
           (even? 1000))",
    );
}

// ============================================================================
// COMPLEX EXAMPLE-DERIVED EQUIVALENCE TESTS
// Extracted from examples/*.sema — testing real-world patterns
// ============================================================================

// --- Functional patterns: currying, composition, BST, collatz ---

#[test]
fn test_complex_currying() {
    assert_evals(
        "(begin
           (define (curry2 f) (fn (a) (fn (b) (f a b))))
           (define add (curry2 +))
           (define add5 (add 5))
           (list (add5 3) (add5 10) (((curry2 *) 3) 7)))",
    );
}

#[test]
fn test_complex_pipe_composition() {
    assert_evals(
        "(begin
           (define (pipe . fns)
             (foldl (fn (acc f) (fn (x) (f (acc x)))) (fn (x) x) fns))
           (define process
             (pipe (fn (x) (* x 2)) (fn (x) (+ x 10)) (fn (x) (* x x))))
           (process 5))",
    );
}

#[test]
fn test_complex_bst_insert_inorder() {
    assert_evals(
        "(begin
           (define (make-tree val left right) (list val left right))
           (define (tree-val t) (first t))
           (define (tree-left t) (nth t 1))
           (define (tree-right t) (nth t 2))
           (define (tree-insert tree val)
             (if (nil? tree) (make-tree val nil nil)
               (cond
                 ((< val (tree-val tree))
                  (make-tree (tree-val tree) (tree-insert (tree-left tree) val) (tree-right tree)))
                 ((> val (tree-val tree))
                  (make-tree (tree-val tree) (tree-left tree) (tree-insert (tree-right tree) val)))
                 (else tree))))
           (define (tree-inorder tree)
             (if (nil? tree) '()
               (append (tree-inorder (tree-left tree))
                       (list (tree-val tree))
                       (tree-inorder (tree-right tree)))))
           (define bst (foldl tree-insert nil (list 5 3 7 1 4 6 8 2)))
           (tree-inorder bst))",
    );
}

#[test]
fn test_complex_collatz_sequence() {
    assert_evals(
        "(begin
           (define (collatz n)
             (let loop ((x n) (steps 0))
               (cond
                 ((= x 1) steps)
                 ((even? x) (loop (/ x 2) (+ steps 1)))
                 (else (loop (+ (* 3 x) 1) (+ steps 1))))))
           (list (collatz 1) (collatz 7) (collatz 27)))",
    );
}

// --- Scheme classics: car/cdr chains, do loops, sieve ---

#[test]
fn test_complex_caar_cadr_compositions() {
    assert_evals(
        "(begin
           (define nested '((1 2 3) (4 5 6) (7 8 9)))
           (list (caar nested) (cadr nested) (caddr nested) (cadar nested)))",
    );
}

#[test]
fn test_complex_do_loop_factorial() {
    assert_evals(
        "(do ((n 10 (- n 1)) (acc 1 (* acc n)))
           ((= n 0) acc))",
    );
}

#[test]
fn test_complex_sieve_of_eratosthenes() {
    assert_evals(
        "(begin
           (define (sieve-primes n)
             (let loop ((candidates (range 2 n)) (primes '()))
               (if (null? candidates)
                 (reverse primes)
                 (let ((p (first candidates)))
                   (loop
                     (filter (fn (x) (not (= 0 (math/remainder x p)))) (rest candidates))
                     (cons p primes))))))
           (sieve-primes 50))",
    );
}

// --- Data structures: hashmaps, records, sets ---

#[test]
fn test_complex_word_frequency() {
    assert_evals(
        r#"(begin
           (define words (string/split "the cat sat on the mat the cat" " "))
           (define freq
             (foldl (fn (acc w)
               (let ((sym (string->keyword w)))
                 (assoc acc sym (+ 1 (get acc sym 0)))))
               (hash-map)
               words))
           (get freq :the))"#,
    );
}

#[test]
fn test_complex_record_linked_list() {
    assert_evals(
        "(begin
           (define-record-type Cell
             (make-cell head tail) cell?
             (head cell-head) (tail cell-tail))
           (define (cell-from-list items)
             (foldr make-cell nil items))
           (define (cell-to-list lst)
             (if (null? lst) '()
               (cons (cell-head lst) (cell-to-list (cell-tail lst)))))
           (define (cell-map f lst)
             (if (null? lst) nil
               (make-cell (f (cell-head lst)) (cell-map f (cell-tail lst)))))
           (cell-to-list (cell-map (fn (x) (* x 2))
                                    (cell-from-list '(10 20 30 40 50)))))",
    );
}

#[test]
fn test_complex_set_operations() {
    assert_evals(
        "(begin
           (define (set . elems)
             (foldl (fn (s e) (assoc s e #t)) {} elems))
           (define (set/member? s elem) (not (nil? (get s elem))))
           (define (set/union s1 s2)
             (foldl (fn (s e) (assoc s e #t)) s1 (keys s2)))
           (define (set/intersection s1 s2)
             (foldl (fn (s e) (if (set/member? s2 e) (assoc s e #t) s))
                    {} (keys s1)))
           (define s1 (set 1 2 3 4 5))
           (define s2 (set 3 4 5 6 7))
           (sort (keys (set/intersection s1 s2))))",
    );
}

#[test]
fn test_complex_power_set_count() {
    assert_evals(
        "(begin
           (define (power-set items)
             (foldl (fn (subsets elem)
                      (append subsets
                              (map (fn (sub) (cons elem sub)) subsets)))
                    (list '())
                    items))
           (length (power-set '(1 2 3 4))))",
    );
}

// --- Matrix math ---

#[test]
fn test_complex_matrix_multiply() {
    assert_evals(
        "(begin
           (define (transpose m)
             (map (fn (c) (map (fn (row) (nth row c)) m))
                  (range (length (first m)))))
           (define (mat-mul a b)
             (let ((bt (transpose b)))
               (map (fn (row-a)
                 (map (fn (col-b)
                   (foldl + 0 (map * row-a col-b)))
                   bt))
                 a)))
           (mat-mul (list (list 1 2) (list 3 4))
                    (list (list 5 6) (list 7 8))))",
    );
}

// --- Threading macros ---

#[test]
fn test_complex_thread_first() {
    assert_evals(
        "(begin
           (defmacro -> (val . forms)
             (if (null? forms) val
               (let ((form (car forms)) (rest (cdr forms)))
                 (if (list? form)
                   `(-> (,(car form) ,val ,@(cdr form)) ,@rest)
                   `(-> (,form ,val) ,@rest)))))
           (-> 5 (+ 3) (* 2)))",
    );
}

#[test]
fn test_complex_thread_last() {
    assert_evals(
        "(begin
           (defmacro ->> (val . forms)
             (if (null? forms) val
               (let ((form (car forms)) (rest (cdr forms)))
                 (if (list? form)
                   `(->> (,(car form) ,@(cdr form) ,val) ,@rest)
                   `(->> (,form ,val) ,@rest)))))
           (->> (range 1 11) (filter even?) (foldl + 0)))",
    );
}

#[test]
fn test_complex_thread_as() {
    assert_evals(
        "(begin
           (defmacro as-> (val name . forms)
             (if (null? forms) val
               (let ((form (car forms)) (rest (cdr forms)))
                 `(let ((,name ,val))
                    (as-> ,form ,name ,@rest)))))
           (as-> 5 x (+ x 3) (* x x) (- x 1)))",
    );
}

// --- Comprehension macro (deeply nested HOFs + macros) ---

#[test]
fn test_complex_comprehension_cartesian() {
    assert_evals(
        "(begin
           (defmacro for/list (bindings body)
             (define (expand bs)
               (if (null? bs) `(list ,body)
                 (let ((clause (car bs)) (rest (cdr bs)))
                   (let ((var (car clause)) (seq (cadr clause)))
                     `(apply append
                        (map (fn (,var) ,(expand rest)) ,seq))))))
             (expand bindings))
           (for/list ((x (list 1 2 3)) (y (list 10 20))) (+ x y)))",
    );
}

// --- State machines and dispatch ---

#[test]
fn test_complex_fsm_transitions() {
    assert_evals(
        r#"(begin
           (define door-fsm
             (hash-map
               "locked" (hash-map "unlock" "closed")
               "closed" (hash-map "open" "open" "lock" "locked")
               "open"   (hash-map "close" "closed")))
           (define (fsm-run fsm state events)
             (if (null? events) state
               (let* ((event (car events))
                      (next (get (get fsm state) event)))
                 (if (null? next) state
                   (fsm-run fsm next (cdr events))))))
           (fsm-run door-fsm "locked" '("unlock" "open" "close" "lock" "unlock" "open")))"#,
    );
}

#[test]
fn test_complex_multimethod_dispatch() {
    assert_evals(
        r#"(begin
           (define (make-multi dispatch-fn)
             {:dispatch-fn dispatch-fn :methods {} :default nil})
           (define (add-method multi dispatch-val impl)
             (assoc multi :methods (assoc (get multi :methods) dispatch-val impl)))
           (define (invoke multi . args)
             (let* ((dispatch-val (apply (get multi :dispatch-fn) args))
                    (method (get (get multi :methods) dispatch-val)))
               (if (nil? method)
                 (if (nil? (get multi :default))
                   (error (format "no method for ~a" dispatch-val))
                   (apply (get multi :default) args))
                 (apply method args))))

           (define eval-expr
             (add-method
               (add-method
                 (add-method
                   (make-multi (fn (expr)
                     (if (number? expr) :literal (get expr :op))))
                   :literal (fn (expr) expr))
                 :add (fn (e) (+ (invoke eval-expr (get e :left))
                                  (invoke eval-expr (get e :right)))))
               :mul (fn (e) (* (invoke eval-expr (get e :left))
                                (invoke eval-expr (get e :right))))))

           (invoke eval-expr
             {:op :mul
              :left {:op :add :left 3 :right 4}
              :right {:op :add :left 10 :right 2}}))"#,
    );
}

// --- String processing ---

#[test]
fn test_complex_caesar_cipher() {
    assert_evals(
        r#"(begin
           (define (caesar text shift)
             (list->string
               (map (fn (ch)
                 (if (char-alphabetic? ch)
                   (let ((base (if (char-upper-case? ch) 65 97))
                         (code (char->integer ch)))
                     (integer->char (+ base (math/remainder (+ (- code base) shift) 26))))
                   ch))
                 (string/chars text))))
           (caesar (caesar "Hello" 3) 23))"#,
    );
}

#[test]
fn test_complex_roman_numeral_int_to_roman() {
    assert_evals(
        r#"(begin
           (define roman-table
             (list (list 1000 "M") (list 900 "CM") (list 500 "D") (list 400 "CD")
                   (list 100 "C") (list 90 "XC") (list 50 "L") (list 40 "XL")
                   (list 10 "X") (list 9 "IX") (list 5 "V") (list 4 "IV")
                   (list 1 "I")))
           (define (int->roman n)
             (let loop ((n n) (table roman-table) (acc ""))
               (if (or (null? table) (= n 0)) acc
                 (let ((value (first (first table)))
                       (symbol (nth (first table) 1)))
                   (if (>= n value)
                     (loop (- n value) table (string-append acc symbol))
                     (loop n (rest table) acc))))))
           (list (int->roman 42) (int->roman 1999) (int->roman 3999)))"#,
    );
}

// --- Bitwise operations ---

#[test]
fn test_complex_ip_address_packing() {
    assert_evals(
        "(begin
           (define (ip-to-int a b c d)
             (bit/or
               (bit/or (bit/shift-left a 24) (bit/shift-left b 16))
               (bit/or (bit/shift-left c 8) d)))
           (define (int-to-ip n)
             (list (bit/and (bit/shift-right n 24) 255)
                   (bit/and (bit/shift-right n 16) 255)
                   (bit/and (bit/shift-right n 8) 255)
                   (bit/and n 255)))
           (int-to-ip (ip-to-int 192 168 1 100)))",
    );
}

// --- Recursive algorithms ---

#[test]
fn test_complex_hanoi_move_count() {
    assert_evals(
        "(begin
           (define (hanoi-moves n)
             (letrec ((solve (fn (n from to aux count)
               (if (= n 0) count
                 (let* ((c1 (solve (- n 1) from aux to count))
                        (c2 (+ c1 1)))
                   (solve (- n 1) aux to from c2))))))
               (solve n :A :C :B 0)))
           (list (hanoi-moves 1) (hanoi-moves 3) (hanoi-moves 5)))",
    );
}

#[test]
fn test_complex_merge_sort() {
    assert_evals(
        "(begin
           (define (merge xs ys)
             (cond
               ((null? xs) ys)
               ((null? ys) xs)
               ((<= (car xs) (car ys))
                (cons (car xs) (merge (cdr xs) ys)))
               (else
                (cons (car ys) (merge xs (cdr ys))))))
           (define (msort lst)
             (if (<= (length lst) 1) lst
               (let* ((mid (int (/ (length lst) 2)))
                      (left (take mid lst))
                      (right (drop mid lst)))
                 (merge (msort left) (msort right)))))
           (msort (list 38 27 43 3 9 82 10)))",
    );
}

#[test]
fn test_complex_quicksort() {
    assert_evals(
        "(begin
           (define (qsort lst)
             (if (<= (length lst) 1) lst
               (let ((pivot (car lst))
                     (rest (cdr lst)))
                 (append
                   (qsort (filter (fn (x) (< x pivot)) rest))
                   (list pivot)
                   (qsort (filter (fn (x) (>= x pivot)) rest))))))
           (qsort (list 5 2 8 1 9 3 7 4 6)))",
    );
}

// --- Nested named-let (the pattern that was most broken) ---

#[test]
fn test_complex_nested_named_let() {
    // Two nested named-let loops
    assert_evals(
        "(begin
           (define (matrix-to-list rows cols)
             (let outer ((r 0) (result '()))
               (if (= r rows) (reverse result)
                 (outer (+ r 1)
                   (cons
                     (let inner ((c 0) (row '()))
                       (if (= c cols) (reverse row)
                         (inner (+ c 1) (cons (+ (* r cols) c) row))))
                     result)))))
           (matrix-to-list 3 4))",
    );
}

#[test]
fn test_complex_named_let_with_closures() {
    // Named-let body creates closures that capture loop variables
    assert_evals(
        "(begin
           (define (make-adders n)
             (let loop ((i 0) (acc '()))
               (if (= i n) (reverse acc)
                 (loop (+ i 1) (cons (fn (x) (+ x i)) acc)))))
           (define adders (make-adders 5))
           (map (fn (f) (f 100)) adders))",
    );
}

// --- Data pipeline with chained HOFs ---

#[test]
fn test_complex_data_pipeline() {
    assert_evals(
        r#"(begin
           (define records
             (list {:name "Alice" :score 95 :city "Berlin"}
                   {:name "Bob" :score 82 :city "London"}
                   {:name "Charlie" :score 91 :city "Berlin"}
                   {:name "Diana" :score 78 :city "Paris"}
                   {:name "Eve" :score 97 :city "Berlin"}))
           (define (top-scorers records min-score)
             (map (fn (r) (get r :name))
                  (filter (fn (r) (>= (get r :score) min-score)) records)))
           (sort (top-scorers records 90)))"#,
    );
}

// --- Cellular automata (grid-based computation) ---

#[test]
fn test_complex_cellular_automata_rule90() {
    assert_evals(
        "(begin
           (define (rule-to-bits rule)
             (map (fn (i) (if (= 0 (bit/and rule (bit/shift-left 1 i))) 0 1))
                  (range 8)))
           (define (apply-rule rule-bits left center right)
             (nth rule-bits (+ (bit/shift-left left 2)
                               (bit/shift-left center 1) right)))
           (define (next-row rule-bits row)
             (let ((len (length row)))
               (map (fn (i)
                 (let ((left   (if (= i 0) 0 (nth row (- i 1))))
                       (center (nth row i))
                       (right  (if (= i (- len 1)) 0 (nth row (+ i 1)))))
                   (apply-rule rule-bits left center right)))
                 (range len))))
           (define bits (rule-to-bits 90))
           (define row0 (list 0 0 0 0 1 0 0 0 0))
           (define row3
             (let loop ((row row0) (n 3))
               (if (= n 0) row (loop (next-row bits row) (- n 1)))))
           row3)",
    );
}

// --- Lazy streams ---

#[test]
fn test_complex_lazy_stream_take() {
    // stream-cons uses explicit thunks (fn () ...) — per SRFI-41, stream-cons
    // must delay the tail; without a macro, we use explicit thunk functions.
    // Streams are 2-element lists: (value thunk), matching examples/lazy.sema.
    assert_evals(
        "(begin
           (define (stream-cons h t) (list h t))
           (define (stream-car s) (car s))
           (define (stream-cdr s) ((cadr s)))
           (define (stream-take n s)
             (if (or (= n 0) (nil? s)) '()
               (cons (stream-car s) (stream-take (- n 1) (stream-cdr s)))))
           (define (stream-from n) (stream-cons n (fn () (stream-from (+ n 1)))))
           (define (stream-filter pred s)
             (cond
               ((nil? s) nil)
               ((pred (stream-car s))
                (stream-cons (stream-car s) (fn () (stream-filter pred (stream-cdr s)))))
               (else (stream-filter pred (stream-cdr s)))))
           (stream-take 5 (stream-filter even? (stream-from 1))))",
    );
}

// --- Mutual recursion patterns ---

#[test]
fn test_complex_mutual_recursion_even_odd() {
    assert_evals(
        "(begin
           (define (my-even? n) (if (= n 0) #t (my-odd? (- n 1))))
           (define (my-odd? n) (if (= n 0) #f (my-even? (- n 1))))
           (list (my-even? 10) (my-odd? 10) (my-even? 15) (my-odd? 15)))",
    );
}

// --- Deeply nested closures ---

#[test]
fn test_complex_closure_counter_factory() {
    assert_evals(
        "(begin
           (define (make-counter start step)
             (let ((n start))
               (lambda ()
                 (let ((current n))
                   (set! n (+ n step))
                   current))))
           (define c1 (make-counter 0 1))
           (define c2 (make-counter 100 10))
           (list (c1) (c1) (c1) (c2) (c2) (c1)))",
    );
}

#[test]
fn test_complex_closure_memoize() {
    assert_evals(
        "(begin
           (define (memoize f)
             (let ((cache (hash-map)))
               (fn (x)
                 (let ((cached (get cache x)))
                   (if (not (nil? cached)) cached
                     (let ((result (f x)))
                       (set! cache (assoc cache x result))
                       result))))))
           (define fib-memo
             (memoize (fn (n)
               (if (<= n 1) n
                 (+ (fib-memo (- n 1)) (fib-memo (- n 2)))))))
           (list (fib-memo 0) (fib-memo 1) (fib-memo 5) (fib-memo 10)))",
    );
}

// --- Complex exception handling ---

#[test]
fn test_complex_try_catch_in_map() {
    assert_evals(
        r#"(begin
           (define (safe-div a b)
             (try (/ a b) (catch e "error")))
           (map (fn (b) (safe-div 10 b)) (list 2 0 5 0 1)))"#,
    );
}

#[test]
fn test_complex_nested_try_catch() {
    assert_evals(
        r#"(try
           (begin
             (define result
               (try (/ 1 0)
                 (catch e1 (try (throw "nested")
                             (catch e2 "caught-nested")))))
             result)
           (catch outer "outer-caught"))"#,
    );
}

// --- Apply and variadic patterns ---

#[test]
fn test_complex_apply_patterns() {
    assert_evals(
        "(begin
           (define (my-sum . nums) (foldl + 0 nums))
           (list
             (apply + (list 1 2 3 4 5))
             (apply my-sum (list 10 20 30))
             (apply string-append (list \"a\" \"b\" \"c\" \"d\"))))",
    );
}

// --- Accumulator patterns ---

#[test]
fn test_complex_flatten_nested_lists() {
    assert_evals(
        "(begin
           (define (flatten lst)
             (cond
               ((null? lst) '())
               ((list? (car lst))
                (append (flatten (car lst)) (flatten (cdr lst))))
               (else (cons (car lst) (flatten (cdr lst))))))
           (flatten '(1 (2 (3 4) 5) (6 7) 8)))",
    );
}

#[test]
fn test_complex_zip_and_unzip() {
    assert_evals(
        "(begin
           (define (zip . lists)
             (if (any null? lists) '()
               (cons (map car lists) (apply zip (map cdr lists)))))
           (zip '(1 2 3) '(a b c) '(10 20 30)))",
    );
}

// --- Recursive inner defines ---

#[test]
fn test_recursive_inner_define() {
    assert_evals(
        "(define (sum-to n)
           (define (loop i acc)
             (if (> i n) acc (loop (+ i 1) (+ acc i))))
           (loop 1 0))
         (sum-to 10)",
    );
}

#[test]
fn test_recursive_inner_define_with_outer_capture() {
    assert_evals(
        r#"(define (int->hex n)
             (define digits "0123456789ABCDEF")
             (define (digit k) (substring digits k (+ k 1)))
             (define (go x acc)
               (if (= x 0) acc
                 (go (math/quotient x 16)
                     (string-append (digit (math/remainder x 16)) acc))))
             (if (= n 0) "0" (go n "")))
           (int->hex 255)"#,
    );
}

#[test]
fn test_multiple_inner_defines() {
    assert_evals(
        "(define (f x)
           (define a 10)
           (define (add-a y) (+ a y))
           (add-a x))
         (f 5)",
    );
}

// --- Deep let* chains ---

#[test]
fn test_delay_captures_lexical_variables() {
    assert_eq!(
        eval_vm(
            "(begin
               (define (lazy-add a b) (delay (+ (force a) (force b))))
               (define x (lazy-add (delay 1) (delay 2)))
               (force x))"
        ),
        Value::int(3)
    );
}

#[test]
fn test_delay_captures_closure_variable() {
    assert_eq!(
        eval_vm(
            "(begin
               (define (make-lazy x) (delay (* x x)))
               (define p (make-lazy 7))
               (force p))"
        ),
        Value::int(49)
    );
}

#[test]
fn test_complex_deep_let_star() {
    assert_evals(
        "(let* ((a 1)
                (b (+ a 1))
                (c (* b 2))
                (d (- c a))
                (e (+ d c))
                (f (* e a))
                (g (+ f d))
                (h (- g c)))
           (list a b c d e f g h))",
    );
}

// === Promise / delay-force ===
#[test]
fn test_vm_delay_force() {
    assert_evals("(force (delay 42))");
    assert_evals("(force (delay (+ 1 2)))");
    assert_evals("(let ((p (delay (* 6 7)))) (+ (force p) (force p)))");
}

// === Nested maps ===
#[test]
fn test_vm_nested_maps() {
    assert_evals("(get {:a {:b 1}} :a)");
    assert_evals("(get (get {:a {:b 42}} :a) :b)");
    assert_evals("(assoc {:x 1} :y 2)");
}

// === Variadic arithmetic ===
#[test]
fn test_vm_variadic_arithmetic() {
    assert_evals("(+ 1 2 3 4 5)");
    assert_evals("(* 1 2 3 4)");
    assert_evals("(- 10)");
    assert_evals("(- 10 3 2)");
}

// === Multiple return values from begin ===
#[test]
fn test_vm_begin_returns_last() {
    assert_evals("(begin 1 2 3)");
    assert_evals("(begin (define x 5) (+ x 1))");
}

// === Tail-recursive functions ===
#[test]
fn test_vm_tail_recursion_deep() {
    assert_evals("(let loop ((n 1000) (acc 0)) (if (= n 0) acc (loop (- n 1) (+ acc n))))");
}

// === Bytevectors ===
#[test]
fn test_vm_bytevectors() {
    assert_evals("(bytevector 1 2 3)");
    assert_evals("(bytevector-length (bytevector 10 20 30))");
    assert_evals("(bytevector-u8-ref (bytevector 10 20 30) 1)");
}

// === Records ===
#[test]
fn test_vm_records() {
    assert_evals("(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point-x (make-point 3 4)))");
    assert_evals("(begin (define-record-type point (make-point x y) point? (x point-x) (y point-y)) (point? (make-point 1 2)))");
}

// === Threading macros ===
// VM doesn't support this yet — `->` and `->>` are unbound (not built-in, likely require macro import)
// #[test]
// fn test_vm_threading() {
//     assert_evals("(-> 5 (+ 3) (* 2))");
//     assert_evals("(->> (list 1 2 3 4 5) (filter even?) (map (fn (x) (* x x))))");
// }

// === Quasiquote / unquote ===
#[test]
fn test_vm_quasiquote() {
    assert_evals("(let ((x 42)) `(a ,x c))");
    assert_evals("(let ((xs '(1 2 3))) `(a ,@xs b))");
}

// === Error equivalence ===
#[test]
fn test_vm_try_catch_types() {
    assert_evals("(try (/ 1 0) (catch e (get e :type)))");
    assert_evals("(try (+ 1 \"a\") (catch e (get e :type)))");
    assert_evals("(try (throw \"boom\") (catch e (get e :type)))");
}

// === Mutual recursion ===
#[test]
fn test_vm_mutual_recursion() {
    assert_evals("(begin (define (even? n) (if (= n 0) #t (odd? (- n 1)))) (define (odd? n) (if (= n 0) #f (even? (- n 1)))) (even? 10))");
}

// === apply ===
// === Depth limit ===
#[test]
fn test_vm_compiler_depth_limit() {
    // Build a deeply nested Value AST directly to avoid reader stack limits.
    // (begin (begin (begin ... (+ 1 1) ...)))
    let mut expr = Value::list(vec![Value::symbol("+"), Value::int(1), Value::int(1)]);
    for _ in 0..600 {
        expr = Value::list(vec![Value::symbol("begin"), expr]);
    }
    // Run through the VM compilation pipeline directly
    let result = sema_vm::compile_program(&[expr], None);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("depth"), "expected depth error, got: {err}");
}

// === M2: VM-native macro expansion ===
//
// These are *absolute oracle* tests pinned to literal values, run on the VM
// backend. They guard the M2 change that made macro expansion (`apply_macro_vm`)
// and prelude/`defmacro` registration VM-native — the dual-eval suite cannot
// distinguish this on its own because both backends share `load_prelude`.

#[test]
fn vm_prelude_threading_macros_oracle() {
    // -> and ->> are prelude macros; expanding them on the VM must be exact.
    assert_eq!(eval_vm("(-> 5 (+ 3) (* 2))"), Value::int(16));
    assert_eq!(eval_vm("(->> 5 (+ 3) (* 2))"), Value::int(16));
    assert_eq!(
        eval_vm("(-> 10 (- 3) (- 2))"),
        Value::int(5),
        "threading is left-to-right, first-arg insertion"
    );
}

#[test]
fn vm_prelude_binding_macros_oracle() {
    // when-let / if-let short-circuit on `nil` (they test `nil?`, not falsiness).
    assert_eq!(eval_vm("(when-let (x (+ 1 2)) (* x 10))"), Value::int(30));
    assert_eq!(eval_vm("(when-let (x nil) (* x 10))"), Value::nil());
    assert_eq!(eval_vm("(if-let (x 7) (* x 2) -1)"), Value::int(14));
    assert_eq!(eval_vm("(if-let (x nil) (* x 2) -1)"), Value::int(-1));
}

#[test]
fn vm_prelude_loop_macros_oracle() {
    assert_eq!(
        eval_vm("(let ((acc 0)) (dotimes (i 5) (set! acc (+ acc i))) acc)"),
        Value::int(10),
        "dotimes sums 0+1+2+3+4"
    );
    assert_eq!(
        eval_vm("(let ((acc 0)) (for-range (i 1 5) (set! acc (+ acc i))) acc)"),
        Value::int(10),
        "for-range sums 1+2+3+4"
    );
}

#[test]
fn vm_user_macro_calls_global_helper_oracle() {
    // A transformer body that calls a *user-defined* global helper while
    // building the expansion — proves apply_macro_vm roots the VM run at the
    // caller env so user globals (not just builtins) resolve.
    //
    // The VM expands macros ahead-of-time at compile, so the helper must exist
    // at expansion time: we define it on a first eval (persists in the global
    // env), then use the macro on a second eval of the same interpreter.
    let interp = Interpreter::new();
    interp
        .eval_str_compiled("(define (wrap-begin forms) (cons 'begin forms))")
        .expect("define helper");
    interp
        .eval_str_compiled("(defmacro run-all (. body) (wrap-begin body))")
        .expect("define macro");
    interp
        .eval_str_compiled("(define counter 0)")
        .expect("define counter");
    interp
        .eval_str_compiled("(run-all (set! counter (+ counter 1)) (set! counter (+ counter 10)))")
        .expect("use macro");
    assert_eq!(
        interp.eval_str_compiled("counter").expect("read counter"),
        Value::int(11),
        "transformer-built (begin ...) must run both set! forms"
    );
}

#[test]
fn vm_macro_quasiquote_and_gensym_oracle() {
    // Quasiquote + unquote-splicing in a transformer body, plus auto-gensym
    // hygiene across two expansions of the same macro (the tmp# binding must
    // not collide). Both `swap!` uses expand independently and correctly.
    let src = r#"
        (begin
          (defmacro my-swap! (a b)
            `(let ((tmp# ,a))
               (set! ,a ,b)
               (set! ,b tmp#)))
          (define p 1)
          (define q 2)
          (define r 3)
          (define s 4)
          (my-swap! p q)
          (my-swap! r s)
          (list p q r s))
    "#;
    assert_eq!(
        eval_vm(src),
        Value::list(vec![
            Value::int(2),
            Value::int(1),
            Value::int(4),
            Value::int(3)
        ])
    );
}

// === M3: VM-native runtime eval/apply ===

#[test]
fn vm_eval_apply_oracle() {
    assert_eq!(eval_vm("(eval '(+ 1 2))"), Value::int(3));
    assert_eq!(eval_vm("(apply + '(1 2 3 4))"), Value::int(10));
    // A closure created inside the eval'd form runs on the VM.
    assert_eq!(
        eval_vm("(eval '(map (fn (x) (* x x)) '(1 2 3)))"),
        Value::list(vec![Value::int(1), Value::int(4), Value::int(9)])
    );
    // eval'd defines persist in the global env.
    assert_eq!(
        eval_vm("(begin (eval '(define ev-x 7)) (* ev-x 6))"),
        Value::int(42)
    );
}

#[test]
fn vm_eval_is_vm_native_runs_async() {
    // Proof that `__vm-eval` runs on the VM:
    // async/await is a VM-only feature, so this only succeeds if the eval'd form
    // is compiled and executed on the bytecode VM.
    assert_eq!(eval_vm("(eval '(await (async (+ 40 2))))"), Value::int(42));
}
