#![allow(clippy::approx_constant)]
use sema_core::{SemaError, Value};
use sema_eval::Interpreter;

/// Assert that evaluating `input` produces an error whose inner variant matches `SemaError::Arity`.
fn assert_arity_error(input: &str) {
    let err = eval_err(input);
    assert!(
        matches!(err.inner(), SemaError::Arity { .. }),
        "expected Arity error for `{input}`, got: {err}"
    );
}

/// Assert that evaluating `input` produces an error whose inner variant matches `SemaError::Type`.
fn assert_type_error(input: &str) {
    let err = eval_err(input);
    assert!(
        matches!(err.inner(), SemaError::Type { .. }),
        "expected Type error for `{input}`, got: {err}"
    );
}

/// True if `err` (or any error it wraps via `WithTrace`/`UserException`) is a
/// sandbox permission denial — either `PermissionDenied` (missing capability) or
/// `PathDenied` (path outside allowed directories). Structured matching replaces
/// fragile `.contains("Permission denied")` message checks.
fn is_permission_error(err: &SemaError) -> bool {
    matches!(
        err.inner(),
        SemaError::PermissionDenied { .. } | SemaError::PathDenied { .. }
    )
}

/// Assert that `err` is a sandbox permission denial.
fn assert_permission_denied(err: &SemaError) {
    assert!(
        is_permission_error(err),
        "expected PermissionDenied/PathDenied, got: {err}"
    );
}

/// Assert that `err` is specifically a `PathDenied` (path outside the allowed
/// directories), not merely a missing-capability denial.
fn assert_path_denied(err: &SemaError) {
    assert!(
        matches!(err.inner(), SemaError::PathDenied { .. }),
        "expected PathDenied, got: {err}"
    );
}

fn eval(input: &str) -> Value {
    let interp = Interpreter::new();
    interp
        .eval_str(input)
        .unwrap_or_else(|_| panic!("failed to eval: {input}"))
}

fn eval_to_string(input: &str) -> String {
    format!("{}", eval(input))
}

/// Assert that `input` evaluates to a float within `1e-10` of `expected`.
///
/// Preferred over `assert_eq!(eval(..), Value::float(..))` for genuine
/// transcendental/irrational computations (sqrt, pow, trig, exp, hyperbolic,
/// lerp), where bit-exact equality is fragile across libm implementations and
/// rounding paths.
fn assert_float_eq(input: &str, expected: f64) {
    let v = eval(input);
    let f = v
        .as_float()
        .unwrap_or_else(|| panic!("expected float from {input}, got {v}"));
    assert!(
        (f - expected).abs() < 1e-10,
        "{input} = {f}, expected ≈ {expected}"
    );
}

/// Build a path inside the OS temp directory, using forward slashes so it can be
/// embedded directly in Sema source string literals on every platform (Windows
/// `temp_dir()` would otherwise contain backslashes that act as escape chars).
/// Replaces hardcoded `/tmp/...` paths in filesystem tests.
fn temp_path(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Evaluate an expression that yields a path string and normalize OS-specific
/// directory separators to `/` so assertions are platform-agnostic (`path/join`
/// emits `\` on Windows).
fn eval_path(input: &str) -> String {
    let v = eval(input);
    let s = v
        .as_str()
        .unwrap_or_else(|| panic!("expected string path from `{input}`, got: {v}"));
    s.replace('\\', "/")
}

#[test]
fn test_arithmetic() {
    assert_eq!(eval("(+ 1 2)"), Value::int(3));
    assert_eq!(eval("(- 10 3)"), Value::int(7));
    assert_eq!(eval("(* 4 5)"), Value::int(20));
    assert_eq!(eval("(/ 10 2)"), Value::int(5));
    assert_eq!(eval("(mod 10 3)"), Value::int(1));
    assert_eq!(eval("(+ 1 2.0)"), Value::float(3.0));
}

#[test]
fn test_comparison() {
    assert_eq!(eval("(< 1 2)"), Value::bool(true));
    assert_eq!(eval("(> 3 2)"), Value::bool(true));
    assert_eq!(eval("(<= 2 2)"), Value::bool(true));
    assert_eq!(eval("(= 42 42)"), Value::bool(true));
    assert_eq!(eval("(not #f)"), Value::bool(true));
}

#[test]
fn test_define_and_call() {
    assert_eq!(eval("(begin (define x 42) x)"), Value::int(42));
    assert_eq!(
        eval("(begin (define (square x) (* x x)) (square 5))"),
        Value::int(25)
    );
}

#[test]
fn test_defun_alias() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str("(defun square (x) (* x x)) (square 5)")
        .unwrap();
    assert_eq!(result.to_string(), "25");
}

#[test]
fn test_factorial() {
    assert_eq!(
        eval("(begin (define (factorial n) (if (<= n 1) 1 (* n (factorial (- n 1))))) (factorial 10))"),
        Value::int(3628800)
    );
}

#[test]
fn test_stack_overflow_hint() {
    let result = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let interp = Interpreter::new();
            interp
                .eval_str("(define (f x) (+ 1 (f x))) (f 0)")
                .unwrap_err()
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(result.hint().is_some());
    assert!(result.hint().unwrap().contains("recursion"));
}

#[test]
fn test_lambda() {
    assert_eq!(eval("((lambda (x y) (+ x y)) 3 4)"), Value::int(7));
}

#[test]
fn test_let() {
    assert_eq!(eval("(let ((x 10) (y 20)) (+ x y))"), Value::int(30));
}

#[test]
fn test_let_star() {
    assert_eq!(eval("(let* ((x 10) (y (* x 2))) (+ x y))"), Value::int(30));
}

#[test]
fn test_cond() {
    assert_eq!(
        eval("(cond ((= 1 2) 10) ((= 1 1) 20) (else 30))"),
        Value::int(20)
    );
}

#[test]
fn test_and_or() {
    assert_eq!(eval("(and 1 2 3)"), Value::int(3));
    assert_eq!(eval("(and 1 #f 3)"), Value::bool(false));
    assert_eq!(eval("(or #f #f 3)"), Value::int(3));
    assert_eq!(eval("(or 1 2 3)"), Value::int(1));
}

#[test]
fn test_list_operations() {
    assert_eq!(eval("(car (list 1 2 3))"), Value::int(1));
    assert_eq!(eval_to_string("(cdr (list 1 2 3))"), "(2 3)");
    assert_eq!(eval_to_string("(cons 0 (list 1 2))"), "(0 1 2)");
    assert_eq!(eval("(length (list 1 2 3))"), Value::int(3));
    assert_eq!(eval_to_string("(reverse (list 1 2 3))"), "(3 2 1)");
    assert_eq!(
        eval_to_string("(append (list 1 2) (list 3 4))"),
        "(1 2 3 4)"
    );
}

#[test]
fn test_map_filter_fold() {
    assert_eq!(
        eval_to_string("(map (lambda (x) (* x x)) (list 1 2 3))"),
        "(1 4 9)"
    );
    assert_eq!(
        eval_to_string("(filter (lambda (x) (> x 2)) (list 1 2 3 4))"),
        "(3 4)"
    );
    assert_eq!(eval("(foldl + 0 (list 1 2 3 4 5))"), Value::int(15));
}

#[test]
fn test_string_operations() {
    assert_eq!(eval("(string-length \"hello\")"), Value::int(5));
    assert_eq!(
        eval("(string/contains? \"hello world\" \"world\")"),
        Value::bool(true)
    );
    assert_eq!(
        eval_to_string("(string/split \"a,b,c\" \",\")"),
        "(\"a\" \"b\" \"c\")"
    );
}

#[test]
fn test_map_data_structure() {
    assert_eq!(eval("(get {:a 1 :b 2} :a)"), Value::int(1));
    assert_eq!(eval("(:b {:a 1 :b 2})"), Value::int(2));
    assert_eq!(eval("(get (assoc {:a 1} :b 2) :b)"), Value::int(2));
    assert_eq!(eval_to_string("(keys {:a 1 :b 2})"), "(:a :b)");
}

#[test]
fn test_json() {
    // Compare JSON structurally to avoid depending on key order
    let result = eval(r#"(json/encode {:name "test" :val 42})"#);
    let result_json: serde_json::Value = serde_json::from_str(result.as_str().unwrap()).unwrap();
    let expected = serde_json::json!({"name": "test", "val": 42});
    assert_eq!(result_json, expected);
    assert_eq!(
        eval("(get (json/decode \"{\\\"x\\\": 10}\") :x)"),
        Value::int(10)
    );
}

#[test]
fn test_quote() {
    assert_eq!(eval_to_string("(quote (a b c))"), "(a b c)");
    assert_eq!(eval_to_string("'(a b c)"), "(a b c)");
}

#[test]
fn test_quasiquote() {
    assert_eq!(
        eval_to_string("(begin (define x 42) `(a ,x b))"),
        "(a 42 b)"
    );
}

#[test]
fn test_when_unless() {
    assert_eq!(eval("(when #t 42)"), Value::int(42));
    assert_eq!(eval("(when #f 42)"), Value::nil());
    assert_eq!(eval("(unless #f 42)"), Value::int(42));
    assert_eq!(eval("(unless #t 42)"), Value::nil());
}

#[test]
fn test_predicates() {
    assert_eq!(eval("(null? nil)"), Value::bool(true));
    assert_eq!(eval("(null? (list))"), Value::bool(true));
    assert_eq!(eval("(list? (list 1 2))"), Value::bool(true));
    assert_eq!(eval("(number? 42)"), Value::bool(true));
    assert_eq!(eval("(string? \"hi\")"), Value::bool(true));
    assert_eq!(eval("(keyword? :foo)"), Value::bool(true));
    assert_eq!(eval("(map? {:a 1})"), Value::bool(true));
}

#[test]
fn test_set_bang() {
    assert_eq!(eval("(begin (define x 1) (set! x 2) x)"), Value::int(2));
}

#[test]
fn test_begin() {
    assert_eq!(eval("(begin 1 2 3)"), Value::int(3));
}

#[test]
fn test_closures() {
    assert_eq!(
        eval("(begin (define (make-adder n) (lambda (x) (+ n x))) ((make-adder 5) 3))"),
        Value::int(8)
    );
}

#[test]
fn test_range() {
    assert_eq!(eval_to_string("(range 5)"), "(0 1 2 3 4)");
    assert_eq!(eval_to_string("(range 2 5)"), "(2 3 4)");
}

#[test]
fn test_rest_params() {
    assert_eq!(
        eval("(begin (define (sum . args) (foldl + 0 args)) (sum 1 2 3 4 5))"),
        Value::int(15)
    );
}

#[test]
fn test_apply() {
    assert_eq!(eval("(apply + (list 1 2 3))"), Value::int(6));
}

#[test]
fn test_math_functions() {
    assert_eq!(eval("(abs -5)"), Value::int(5));
    assert_eq!(eval("(min 3 1 2)"), Value::int(1));
    assert_eq!(eval("(max 3 1 2)"), Value::int(3));
    // Exactness-preserving (R7RS): a float argument rounds to a float.
    assert_eq!(eval("(floor 3.7)"), Value::float(3.0));
    assert_eq!(eval("(ceil 3.2)"), Value::float(4.0));
}

#[test]
fn test_prompt_and_message() {
    let result = eval("(prompt (system \"You are helpful.\") (user \"Hello\"))");
    assert!(result.as_prompt_rc().is_some());

    let result = eval("(message :user \"Hello\")");
    assert!(result.as_message_rc().is_some());
}

#[test]
fn test_recursive_fibonacci() {
    assert_eq!(
        eval("(begin (define (fib n) (cond ((= n 0) 0) ((= n 1) 1) (else (+ (fib (- n 1)) (fib (- n 2)))))) (fib 10))"),
        Value::int(55)
    );
}

#[test]
fn test_higher_order() {
    assert_eq!(
        eval("(begin (define (compose f g) (lambda (x) (f (g x)))) (define inc (lambda (x) (+ x 1))) (define dbl (lambda (x) (* x 2))) ((compose dbl inc) 5))"),
        Value::int(12)
    );
}

#[test]
fn test_sort() {
    assert_eq!(
        eval_to_string("(sort (list 3 1 4 1 5 9 2 6))"),
        "(1 1 2 3 4 5 6 9)"
    );
}

#[test]
fn test_format() {
    assert_eq!(
        eval("(format \"Hello ~a, you are ~a\" \"world\" 42)"),
        Value::string("Hello world, you are 42")
    );
}

#[test]
fn test_string_conversions() {
    assert_eq!(eval("(string->number \"42\")"), Value::int(42));
    assert_eq!(eval("(string->number \"3.14\")"), Value::float(3.14));
    assert_eq!(eval("(number->string 42)"), Value::string("42"));
}

#[test]
fn test_deftool() {
    let result = eval(
        r#"
        (begin
          (deftool add-numbers
            "Add two numbers"
            {:a {:type :number :description "First number"}
             :b {:type :number :description "Second number"}}
            (lambda (a b) (+ a b)))
          add-numbers)
    "#,
    );
    assert!(result.as_tool_def_rc().is_some());
}

#[test]
fn test_defagent() {
    let result = eval(
        r#"
        (begin
          (deftool greet
            "Greet someone"
            {:name {:type :string}}
            (lambda (name) (string-append "Hello, " name "!")))
          (defagent greeter {:system "You greet people."
                             :tools [greet]
                             :max-turns 5})
          greeter)
    "#,
    );
    assert!(result.as_agent_rc().is_some());
}

#[test]
fn test_load_special_form() {
    // Write a temp file and load it
    let path = temp_path("sema-test-load.sema");
    eval(&format!(
        r#"(file/write "{path}" "(define loaded-value 42)")"#
    ));
    let result = eval(&format!(
        r#"
        (begin
          (load "{path}")
          loaded-value)
    "#
    ));
    assert_eq!(result, Value::int(42));
}

#[test]
fn test_load_recursive_fn_reads_live_global_after_cross_fn_set() {
    // Regression for issue #82: a self-recursive function DEFINED IN A `load`ed
    // unit that directly reads a mutable global must observe a cross-function
    // `set!` to that global performed mid-loop. The write itself lands (fresh
    // calls see it), but a broken VM kept the in-flight recursive reader pinned
    // to the pre-`set!` value forever — an infinite loop (the shape of every
    // "loop until a callback flips a flag" event loop, e.g. a TUI `/quit`).
    //
    // Root cause: each top-level form of a `load`ed unit runs on its own per-form
    // VM over a clone of the caller env, and `Env::clone` used to fork the
    // version cell the inline global cache is keyed on. So `run` and `quit!`
    // captured home-globals handles with *independent* version cells that share
    // one bindings map: `quit!`'s `set!` bumped only its own cell, while `run`'s
    // recursive reader kept hitting its own never-bumped cache entry and served
    // the pre-`set!` value forever.
    //
    // The manifestation is inline-cache-slot-layout sensitive (adding a colliding
    // global read to the reproducing functions accidentally masks it), so this
    // test keeps the exact reported repro shape (named-let + a direct `unless`
    // read + a trailing form) and bounds a regressed hang with an eval step limit
    // rather than by editing the loop — a broken build errors out fast instead of
    // wedging CI.
    let dir = std::env::temp_dir().join(format!("sema-issue82-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lib = dir.join("lib.sema");
    std::fs::write(
        &lib,
        r#"
(define *sq* #f)
(define (quit!) (set! *sq* #t))
(define (advance i) (when (= i 2) (quit!)))
(define (run)
  (let loop ((i 0))
    (advance i)
    (unless *sq* (loop (+ i 1))))
  'exited-ok)
"#,
    )
    .unwrap();
    let lib_path = lib.to_string_lossy().replace('\\', "/");

    let interp = Interpreter::new();
    // Bound a regressed infinite loop: the guard fires on the named-let's
    // backward jump, so a broken build hits the limit and errors rather than
    // hanging. The fixed build exits after 3 iterations, far under the limit.
    interp.ctx.set_eval_step_limit(1_000_000);
    let result = interp
        .eval_str(&format!("(load \"{lib_path}\")\n(run)"))
        .expect("issue #82: recursive reader in a loaded unit never observed the cross-function set! (step limit tripped)");
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(result, eval("'exited-ok"));
}

#[test]
fn test_defmacro() {
    assert_eq!(
        eval(
            r#"
            (begin
              (defmacro my-if (cond then else)
                (list 'if cond then else))
              (my-if #t 1 2))
        "#
        ),
        Value::int(1)
    );
}

#[test]
fn test_llm_similarity() {
    // llm/similarity should compute cosine similarity
    assert_eq!(
        eval_to_string("(llm/similarity '(1.0 0.0 0.0) '(1.0 0.0 0.0))"),
        "1.0"
    );
    assert_eq!(
        eval_to_string("(llm/similarity '(1.0 0.0) '(0.0 1.0))"),
        "0.0"
    );
}

#[test]
fn test_pricing() {
    // llm/set-pricing should not error
    assert_eq!(
        eval_to_string("(llm/set-pricing \"my-model\" 1.0 2.0)"),
        "nil"
    );
}

#[test]
fn test_llm_reset_usage() {
    assert_eq!(eval_to_string("(llm/reset-usage)"), "nil");
}

#[test]
fn test_multi_list_map() {
    assert_eq!(eval_to_string("(map + '(1 2 3) '(10 20 30))"), "(11 22 33)");
    // Shortest wins
    assert_eq!(eval_to_string("(map + '(1 2 3) '(10 20))"), "(11 22)");
}

#[test]
fn test_take_drop() {
    assert_eq!(eval_to_string("(take 2 '(1 2 3 4))"), "(1 2)");
    assert_eq!(eval_to_string("(drop 2 '(1 2 3 4))"), "(3 4)");
    assert_eq!(eval_to_string("(take 10 '(1 2))"), "(1 2)");
    assert_eq!(eval_to_string("(drop 10 '(1 2))"), "()");
}

#[test]
fn test_last() {
    assert_eq!(eval("(last '(1 2 3))"), Value::int(3));
    assert_eq!(eval("(last '())"), Value::nil());
}

#[test]
fn test_zip() {
    assert_eq!(eval_to_string("(zip '(1 2) '(a b))"), "((1 a) (2 b))");
    assert_eq!(
        eval_to_string("(zip '(1 2 3) '(a b) '(x y))"),
        "((1 a x) (2 b y))"
    );
}

#[test]
fn test_flatten() {
    assert_eq!(
        eval_to_string("(flatten '((1 2) (3 4) (5)))"),
        "(1 2 3 4 5)"
    );
    assert_eq!(eval_to_string("(flatten '(1 (2 3) 4))"), "(1 2 3 4)");
}

#[test]
fn test_member() {
    assert_eq!(eval_to_string("(member 3 '(1 2 3 4 5))"), "(3 4 5)");
    assert_eq!(eval("(member 9 '(1 2 3))"), Value::bool(false));
}

#[test]
fn test_any_every() {
    assert_eq!(
        eval("(any (lambda (x) (> x 3)) '(1 2 3 4 5))"),
        Value::bool(true)
    );
    assert_eq!(
        eval("(any (lambda (x) (> x 10)) '(1 2 3))"),
        Value::bool(false)
    );
    assert_eq!(
        eval("(every (lambda (x) (> x 0)) '(1 2 3))"),
        Value::bool(true)
    );
    assert_eq!(
        eval("(every (lambda (x) (> x 2)) '(1 2 3))"),
        Value::bool(false)
    );
}

#[test]
fn test_reduce() {
    assert_eq!(eval("(reduce + '(1 2 3 4 5))"), Value::int(15));
    assert_eq!(eval("(reduce * '(1 2 3 4))"), Value::int(24));
}

#[test]
fn test_partition() {
    assert_eq!(
        eval_to_string("(partition (lambda (x) (> x 2)) '(1 2 3 4 5))"),
        "((3 4 5) (1 2))"
    );
}

#[test]
fn test_foldr() {
    assert_eq!(eval_to_string("(foldr cons '() '(1 2 3))"), "(1 2 3)");
    assert_eq!(eval("(foldr + 0 '(1 2 3 4 5))"), Value::int(15));
}

#[test]
fn test_named_let_sum() {
    assert_eq!(
        eval("(let loop ((i 1) (acc 0)) (if (> i 10) acc (loop (+ i 1) (+ acc i))))"),
        Value::int(55)
    );
}

#[test]
fn test_named_let_fib() {
    assert_eq!(
        eval("(let fib ((n 10) (a 0) (b 1)) (if (= n 0) a (fib (- n 1) b (+ a b))))"),
        Value::int(55)
    );
}

#[test]
fn test_letrec() {
    assert_eq!(
        eval(
            r#"
            (letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1)))))
                     (odd?  (lambda (n) (if (= n 0) #f (even? (- n 1))))))
              (even? 10))
        "#
        ),
        Value::bool(true)
    );
    assert_eq!(
        eval(
            r#"
            (letrec ((even? (lambda (n) (if (= n 0) #t (odd? (- n 1)))))
                     (odd?  (lambda (n) (if (= n 0) #f (even? (- n 1))))))
              (odd? 7))
        "#
        ),
        Value::bool(true)
    );
}

#[test]
fn test_try_catch_error() {
    assert_eq!(
        eval(r#"(try (error "boom") (catch e (:message e)))"#),
        Value::string("boom")
    );
}

#[test]
fn test_try_no_error() {
    assert_eq!(eval("(try 42 (catch e 0))"), Value::int(42));
}

#[test]
fn test_throw_catch() {
    assert_eq!(
        eval("(try (throw 99) (catch e (:value e)))"),
        Value::int(99)
    );
}

#[test]
fn test_catch_type_error() {
    assert_eq!(
        eval(r#"(try (+ 1 "nope") (catch e (:type e)))"#),
        Value::keyword("type-error")
    );
}

#[test]
fn test_catch_unbound() {
    assert_eq!(
        eval(r#"(try no-such-var (catch e (:type e)))"#),
        Value::keyword("unbound")
    );
}

#[test]
fn test_nested_try() {
    assert_eq!(
        eval(
            r#"
            (try
              (try (throw 1) (catch e (throw (+ (:value e) 1))))
              (catch e (:value e)))
        "#
        ),
        Value::int(2)
    );
}

fn eval_err(input: &str) -> SemaError {
    let interp = Interpreter::new();
    interp.eval_str(input).unwrap_err()
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sema-{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

fn lisp_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[test]
fn test_module_import() {
    // Write a module file
    let path = temp_path("sema-test-module.sema");
    eval(&format!(
        r#"(file/write "{path}" "(module math (export add square) (define (add a b) (+ a b)) (define (square x) (* x x)) (define internal 42))")"#,
    ));
    // Import and use
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{path}")
              (add 3 4))
        "#
        )),
        Value::int(7)
    );
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{path}")
              (square 5))
        "#
        )),
        Value::int(25)
    );
    // internal should NOT be exported
    let err = eval_err(&format!(
        r#"
        (begin
          (import "{path}")
          internal)
    "#,
    ));
    assert!(matches!(err.inner(), SemaError::Unbound(_)));
}

#[test]
fn test_selective_import() {
    let path = temp_path("sema-test-sel.sema");
    eval(&format!(
        r#"(file/write "{path}" "(module m (export foo bar) (define (foo) 1) (define (bar) 2))")"#,
    ));
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{path}" foo)
              (foo))
        "#
        )),
        Value::int(1)
    );
    // bar should not be imported
    let err = eval_err(&format!(
        r#"
        (begin
          (import "{path}" foo)
          (bar))
    "#,
    ));
    assert!(matches!(err.inner(), SemaError::Unbound(_)));
}

#[test]
fn test_module_cache() {
    let path = temp_path("sema-test-cache.sema");
    eval(&format!(
        r#"(file/write "{path}" "(module c (export val) (define val 99))")"#
    ));
    // Import twice — should work fine (cached)
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{path}")
              (import "{path}")
              val)
        "#
        )),
        Value::int(99)
    );
}

#[test]
fn test_nested_import_does_not_leak_non_exported_bindings() {
    let dir = unique_temp_dir("nested-import-exports");
    let module_b = dir.join("b.sema");
    let module_a = dir.join("a.sema");

    std::fs::write(
        &module_b,
        "(module b (export z) (define z 3) (define hidden-b 99))",
    )
    .unwrap();
    std::fs::write(
        &module_a,
        format!(
            "(module a (export x) (define x 1) (define y 2) (import \"{}\"))",
            lisp_path(&module_b)
        ),
    )
    .unwrap();

    let err = eval_err(&format!(
        r#"
        (begin
          (import "{}")
          y)
    "#,
        lisp_path(&module_a)
    ));
    assert!(
        matches!(err.inner(), SemaError::Unbound(_)),
        "expected unbound y, got: {}",
        err.inner()
    );

    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{}")
              x)
        "#,
            lisp_path(&module_a)
        )),
        Value::int(1)
    );

    let err = eval_err(&format!(
        r#"
        (begin
          (import "{}")
          z)
    "#,
        lisp_path(&module_a)
    ));
    assert!(
        matches!(err.inner(), SemaError::Unbound(_)),
        "expected unbound z, got: {}",
        err.inner()
    );
}

/// Fixture for the open-upvalue dispatch regression tests: a `registry` module
/// whose `reg/emit` dispatches handler closures through a HOF (`for-each`),
/// and a `runner` module that invokes thunks through `map`. The combination
/// drives callbacks through the native→VM re-entry path with frames from
/// several modules interleaved, so an earlier callback's frame activation
/// re-points the VM's live `globals`/`functions` before the next handler is
/// dispatched from the same paused native. Such a dispatch must run nested on
/// the VM that owns the handlers' open upvalue cells — a callback strayed onto
/// a fresh VM dereferences those cells against the wrong stack: out of bounds
/// (process panic at LOAD_UPVALUE) when the index is high, a silent wrong-slot
/// read/write when it happens to be in bounds. The `set!` write-back
/// assertions below catch the silent variant.
fn write_dispatch_fixture_modules(dir: &std::path::Path) -> (String, String) {
    let registry = dir.join("registry.sema");
    let runner = dir.join("runner.sema");
    std::fs::write(
        &registry,
        r#"(module registry
             (export reg/set! reg/emit)
             (define handlers {})
             (defun reg/set! (m) (set! handlers m))
             (defun reg/emit (ev)
               (for-each (fn (entry) ((cadr entry) ev))
                         (map/entries handlers))))"#,
    )
    .unwrap();
    std::fs::write(
        &runner,
        r#"(module runner
             (export run/all)
             (define (run-one f) (try (begin (f) #t) (catch e e)))
             (defun run/all (. fns) (map run-one fns)))"#,
    )
    .unwrap();
    (lisp_path(&registry), lisp_path(&runner))
}

#[test]
fn test_hof_dispatch_open_upvalue_deep_nesting_no_panic() {
    // Deep variant: the capturing closure's frame sits under two layers of
    // nested HOF dispatch, so its open upvalue's stack index is far beyond
    // what a fresh fallback VM's stack would hold.
    let dir = unique_temp_dir("hof-upvalue-deep");
    let (registry, runner) = write_dispatch_fixture_modules(&dir);
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{registry}")
              (import "{runner}")
              (define captured (list))
              (defun capture (ev) (set! captured (append captured (list ev))))
              (define result -1)
              (defun t ()
                (define local-events (list))
                (define (local-handler ev)
                  (set! local-events (append local-events (list ev))))
                ;; :a sorts before :b — the first dispatch (main-env `capture`)
                ;; re-points the VM's live globals before :b is dispatched.
                (reg/set! {{:a capture :b local-handler}})
                (reg/emit {{:msg 1}})
                (set! result (length local-events)))
              (run/all t)
              (list result (length captured)))
        "#
        )),
        eval("'(1 1)"),
        "set! through the handler's open upvalue must flow back to the defining frame"
    );
}

#[test]
fn test_hof_dispatch_open_upvalue_shallow_write_back() {
    // Shallow variant: the open upvalue's stack index is small enough to be
    // in bounds on a fresh VM's stack, where a strayed dispatch reads and
    // writes the wrong slot with no error — only the write-back assertion
    // catches it.
    let dir = unique_temp_dir("hof-upvalue-shallow");
    let (registry, _) = write_dispatch_fixture_modules(&dir);
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{registry}")
              (define captured (list))
              (defun capture (ev) (set! captured (append captured (list ev))))
              (defun t ()
                (define local-events (list))
                (define (local-handler ev)
                  (set! local-events (append local-events (list ev))))
                (reg/set! {{:a capture :b local-handler}})
                (reg/emit {{:msg 1}})
                (length local-events))
              (t))
        "#
        )),
        Value::int(1),
        "handler must observe and mutate the real captured local, not a foreign stack slot"
    );
}

#[test]
fn test_imported_hof_wrapper_set_write_back() {
    // A caller-supplied closure dispatched as the direct argument of an
    // imported module's HOF wrapper. The module env's parent is a fresh Rc
    // clone of the root env, so an identity test on the root Env Rc can never
    // match across the import boundary — compatibility must key on the root's
    // shared bindings. A dispatch strayed onto a fresh VM snapshots the
    // closure's open upvalue and the `set!` never reaches the caller's slot.
    let dir = unique_temp_dir("imported-hof-write-back");
    let hof = dir.join("hof.sema");
    std::fs::write(
        &hof,
        "(module hof (export hof/each) (defun hof/each (f xs) (for-each f xs)))",
    )
    .unwrap();
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{}")
              (let ((n 0))
                (hof/each (fn (x) (set! n (+ n 1))) (list 1 2 3))
                n))
        "#,
            lisp_path(&hof)
        )),
        Value::int(3),
        "set! through an imported HOF wrapper must flow back to the caller's local"
    );
}

#[test]
fn test_imported_hof_transitive_closure_no_slot_clobber() {
    // A closure reached *transitively* by the dispatched callback (not the
    // callback itself) writes through its open upvalue. On a strayed fresh VM
    // the write resolves against the callback frame's slots instead, silently
    // clobbering an unrelated local — both bystander integrity and the real
    // write-back are asserted.
    let dir = unique_temp_dir("imported-hof-clobber");
    let hof = dir.join("hof.sema");
    std::fs::write(
        &hof,
        "(module hof (export hof/each) (defun hof/each (f xs) (for-each f xs)))",
    )
    .unwrap();
    assert_eq!(
        eval(&format!(
            r#"
            (begin
              (import "{}")
              (define observed nil)
              (define (outer)
                (let ((secret 111))
                  (let ((writer (fn () (set! secret 222))))
                    (hof/each (fn (x)
                                (let ((a :a) (b :b) (c :c))
                                  (writer)
                                  (set! observed (list a b c))))
                              (list 0))
                    secret)))
              (list (outer) observed))
        "#,
            lisp_path(&hof)
        )),
        eval("'(222 (:a :b :c))"),
        "transitive closure writes must reach their own slot and no other"
    );
}

#[test]
fn test_cyclic_import_returns_error_instead_of_stack_overflow() {
    let dir = unique_temp_dir("cyclic-import");
    let module_a = dir.join("a.sema");
    let module_b = dir.join("b.sema");

    std::fs::write(
        &module_a,
        format!(
            "(module a (export x) (define x 1) (import \"{}\"))",
            lisp_path(&module_b)
        ),
    )
    .unwrap();
    std::fs::write(
        &module_b,
        format!(
            "(module b (export y) (define y 2) (import \"{}\"))",
            lisp_path(&module_a)
        ),
    )
    .unwrap();

    let err = eval_err(&format!(r#"(import "{}")"#, lisp_path(&module_a)));
    let msg = format!("{}", err.inner());
    assert!(
        msg.contains("cyclic import detected"),
        "expected cyclic import error, got: {msg}"
    );
}

#[test]
fn test_module_cache_isolation_between_interpreters() {
    let dir = unique_temp_dir("module-cache-isolation");
    let module_path = dir.join("cache.sema");

    std::fs::write(&module_path, "(module c (export val) (define val 1))").unwrap();

    let interp1 = Interpreter::new();
    assert_eq!(
        interp1
            .eval_str(&format!(
                r#"(begin (import "{}") val)"#,
                lisp_path(&module_path)
            ))
            .unwrap(),
        Value::int(1)
    );

    std::fs::write(&module_path, "(module c (export val) (define val 2))").unwrap();

    let interp2 = Interpreter::new();
    assert_eq!(
        interp2
            .eval_str(&format!(
                r#"(begin (import "{}") val)"#,
                lisp_path(&module_path)
            ))
            .unwrap(),
        Value::int(2)
    );
}

#[test]
fn test_load_pops_file_context_even_if_file_disappears() {
    let dir = unique_temp_dir("load-pop-context");
    let script = dir.join("self-delete.sema");
    let script_path = lisp_path(&script);

    std::fs::write(
        &script,
        format!(r#"(begin (file/delete "{}") 1)"#, script_path),
    )
    .unwrap();

    let interp = Interpreter::new();
    assert_eq!(
        interp
            .eval_str(&format!(r#"(load "{}")"#, script_path))
            .unwrap(),
        Value::int(1)
    );
    assert!(
        interp.ctx.current_file_path().is_none(),
        "current file context should be cleared after load"
    );
}

#[test]
fn test_case() {
    assert_eq!(
        eval(r#"(case (+ 1 1) ((1) "one") ((2 3) "two-or-three") (else "other"))"#),
        Value::string("two-or-three")
    );
    assert_eq!(
        eval(r#"(case :b ((:a) 1) ((:b :c) 2) (else 3))"#),
        Value::int(2)
    );
    assert_eq!(
        eval(r#"(case 99 ((1) "one") (else "other"))"#),
        Value::string("other")
    );
    // No match, no else
    assert_eq!(eval(r#"(case 99 ((1) "one") ((2) "two"))"#), Value::nil());
}

#[test]
fn test_eval_special_form() {
    assert_eq!(eval("(eval '(+ 1 2))"), Value::int(3));
    assert_eq!(eval(r#"(eval (read "(* 6 7)"))"#), Value::int(42));
}

#[test]
fn test_read_builtin() {
    assert_eq!(eval(r#"(read "42")"#), Value::int(42));
    assert_eq!(eval_to_string(r#"(read "(+ 1 2)")"#), "(+ 1 2)");
    assert_eq!(eval_to_string(r#"(read-many "1 2 3")"#), "(1 2 3)");
}

#[test]
fn test_type_conversions() {
    assert_eq!(eval_to_string(r#"(string->symbol "foo")"#), "foo");
    assert_eq!(eval(r#"(symbol->string 'foo)"#), Value::string("foo"));
    assert_eq!(eval_to_string(r#"(string->keyword "bar")"#), ":bar");
    assert_eq!(eval(r#"(keyword->string :bar)"#), Value::string("bar"));
}

#[test]
fn test_gensym() {
    // gensym returns a symbol
    let result = eval("(gensym)");
    assert!(result.is_symbol());
    // gensym with prefix
    let result = eval(r#"(gensym "tmp")"#);
    if let Some(s) = result.as_symbol_spur() {
        assert!(sema_core::resolve(s).starts_with("tmp__"));
    } else {
        panic!("expected symbol");
    }
    // Two gensyms are different
    assert_eq!(
        eval("(begin (define a (gensym)) (define b (gensym)) (= a b))"),
        Value::bool(false)
    );
}

#[test]
fn test_macroexpand() {
    assert_eq!(
        eval_to_string(
            r#"
            (begin
              (defmacro my-if (c t e) (list 'if c t e))
              (macroexpand '(my-if #t 1 2)))
        "#
        ),
        "(if #t 1 2)"
    );
    // Non-macro form returned as-is
    assert_eq!(eval("(macroexpand '(+ 1 2))"), eval("'(+ 1 2)"));
}

#[test]
fn test_file_operations() {
    let dir = temp_path("sema-test-fileops");
    let dir = dir.as_str();
    // Clean up from previous runs
    let _ = std::fs::remove_dir_all(dir);

    eval(&format!(r#"(file/mkdir "{dir}/sub")"#));
    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}/sub")"#)),
        Value::bool(true)
    );

    eval(&format!(r#"(file/write "{dir}/a.txt" "hello")"#));
    eval(&format!(r#"(file/append "{dir}/a.txt" " world")"#));
    assert_eq!(
        eval(&format!(r#"(file/read "{dir}/a.txt")"#)),
        Value::string("hello world")
    );

    eval(&format!(r#"(file/rename "{dir}/a.txt" "{dir}/b.txt")"#));
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/b.txt")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/a.txt")"#)),
        Value::bool(false)
    );

    let info = eval(&format!(r#"(file/info "{dir}/b.txt")"#));
    assert!(info.is_map());

    eval(&format!(r#"(file/delete "{dir}/b.txt")"#));
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/b.txt")"#)),
        Value::bool(false)
    );

    // file/list
    eval(&format!(r#"(file/write "{dir}/x.txt" "x")"#));
    eval(&format!(r#"(file/write "{dir}/y.txt" "y")"#));
    let listing = eval(&format!(r#"(file/list "{dir}")"#));
    if let Some(items) = listing.as_list() {
        assert!(items.len() >= 2); // sub, x.txt, y.txt
    } else {
        panic!("expected list");
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_path_operations() {
    assert_eq!(
        eval_path(r#"(path/join "usr" "local" "bin")"#),
        "usr/local/bin"
    );
    assert_eq!(eval(r#"(path/dirname "/a/b/c")"#), Value::string("/a/b"));
    assert_eq!(
        eval(r#"(path/basename "/a/b/c.txt")"#),
        Value::string("c.txt")
    );
    assert_eq!(eval(r#"(path/extension "foo.txt")"#), Value::string("txt"));
    assert_eq!(eval(r#"(path/extension "no-ext")"#), Value::string(""));
    // path/absolute returns a real path — just check it doesn't error
    let result = eval(r#"(path/absolute ".")"#);
    assert!(result.is_string());
}

#[test]
fn test_regex_match() {
    assert_eq!(eval(r#"(regex/match? "\\d+" "abc123")"#), Value::bool(true));
    assert_eq!(
        eval(r#"(regex/match? "^\\d+$" "abc123")"#),
        Value::bool(false)
    );
}

#[test]
fn test_regex_capture() {
    let result = eval(r#"(regex/match "(\\d+)-(\\w+)" "item-42-foo")"#);
    if let Some(m) = result.as_map_rc() {
        assert_eq!(
            m.get(&Value::keyword("match")),
            Some(&Value::string("42-foo"))
        );
    } else {
        panic!("expected map, got {result}");
    }
}

#[test]
fn test_regex_find_all() {
    assert_eq!(
        eval_to_string(r#"(regex/find-all "\\d+" "a1b2c3")"#),
        r#"("1" "2" "3")"#
    );
}

#[test]
fn test_regex_replace() {
    assert_eq!(
        eval(r#"(regex/replace "\\d+" "X" "a1b2c3")"#),
        Value::string("aXb2c3")
    );
    assert_eq!(
        eval(r#"(regex/replace-all "\\d+" "X" "a1b2c3")"#),
        Value::string("aXbXcX")
    );
}

#[test]
fn test_regex_split() {
    assert_eq!(
        eval_to_string(r#"(regex/split "[,;]" "a,b;c,d")"#),
        r#"("a" "b" "c" "d")"#
    );
}

#[test]
fn test_http_get_wrong_arity() {
    assert_arity_error(r#"(http/get)"#);
}

#[test]
fn test_http_post_wrong_arity() {
    assert_arity_error(r#"(http/post "https://httpbin.org/post")"#);
}

// Any valid HTTP token method is accepted (QUERY/RFC 10008, OPTIONS, custom);
// only a method with illegal characters errors — at build time, no request made.
#[test]
fn test_http_request_invalid_method() {
    let err = eval_err(r#"(http/request "BAD METHOD" "http://127.0.0.1:1/x")"#);
    assert!(
        matches!(err.inner(), SemaError::Eval(_)),
        "expected Eval error for an invalid method token, got: {err}"
    );
}

// Non-string URL → type error
#[test]
fn test_http_get_non_string_url() {
    assert_type_error(r#"(http/get 42)"#);
}

#[test]
fn test_http_post_non_string_url() {
    assert_type_error(r#"(http/post 42 "body")"#);
}

#[test]
fn test_http_request_non_string_method() {
    assert_type_error(r#"(http/request 42 "https://httpbin.org/get")"#);
}

// Too many args (upper arity bounds)
#[test]
fn test_http_get_too_many_args() {
    assert_arity_error(r#"(http/get "url" {} "extra")"#);
}

#[test]
fn test_http_post_too_many_args() {
    assert_arity_error(r#"(http/post "url" "body" {} "extra")"#);
}

#[test]
fn test_http_put_wrong_arity() {
    assert_arity_error(r#"(http/put "url")"#);
}

#[test]
fn test_http_delete_wrong_arity() {
    assert_arity_error(r#"(http/delete)"#);
}

#[test]
fn test_http_request_too_few_args() {
    let _err = eval_err(r#"(http/request "GET")"#);
}

#[test]
fn test_http_request_too_many_args() {
    let _err = eval_err(r#"(http/request "GET" "url" {} "body" "extra")"#);
}

// Expanded test suite — covering implemented but previously untested
// features, edge cases, and patterns from mal/Chibi/R7RS test suites.

#[test]
fn test_bool_predicate() {
    assert_eq!(eval("(bool? #t)"), Value::bool(true));
    assert_eq!(eval("(bool? #f)"), Value::bool(true));
    assert_eq!(eval("(bool? 0)"), Value::bool(false));
    assert_eq!(eval("(bool? nil)"), Value::bool(false));
    assert_eq!(eval("(bool? \"true\")"), Value::bool(false));
}

#[test]
fn test_nil_predicate() {
    assert_eq!(eval("(nil? nil)"), Value::bool(true));
    assert_eq!(eval("(nil? #f)"), Value::bool(false));
    assert_eq!(eval("(nil? 0)"), Value::bool(false));
    assert_eq!(eval("(nil? (list))"), Value::bool(false)); // empty list is NOT nil
    assert_eq!(eval("(nil? \"\")"), Value::bool(false));
}

#[test]
fn test_fn_predicate() {
    assert_eq!(eval("(fn? +)"), Value::bool(true));
    assert_eq!(eval("(fn? car)"), Value::bool(true));
    assert_eq!(eval("(fn? (lambda (x) x))"), Value::bool(true));
    assert_eq!(eval("(fn? 42)"), Value::bool(false));
    assert_eq!(eval("(fn? :foo)"), Value::bool(false));
}

#[test]
fn test_type_function() {
    assert_eq!(eval("(type 42)"), Value::keyword("int"));
    assert_eq!(eval("(type 3.14)"), Value::keyword("float"));
    assert_eq!(eval("(type \"hi\")"), Value::keyword("string"));
    assert_eq!(eval("(type :foo)"), Value::keyword("keyword"));
    assert_eq!(eval("(type 'sym)"), Value::keyword("symbol"));
    assert_eq!(eval("(type (list 1 2))"), Value::keyword("list"));
    assert_eq!(eval("(type [1 2])"), Value::keyword("vector"));
    assert_eq!(eval("(type {:a 1})"), Value::keyword("map"));
    assert_eq!(eval("(type #t)"), Value::keyword("bool"));
    assert_eq!(eval("(type nil)"), Value::keyword("nil"));
    assert_eq!(eval("(type +)"), Value::keyword("native-fn"));
    // A user lambda is a native-fn-wrapped VM closure under the hood, but the
    // wrapper is marked (NativeFn::is_closure) so `type` reports :lambda.
    assert_eq!(eval("(type (lambda (x) x))"), Value::keyword("lambda"));
}

#[test]
fn test_integer_float_predicates() {
    assert_eq!(eval("(integer? 42)"), Value::bool(true));
    assert_eq!(eval("(integer? 3.14)"), Value::bool(false));
    assert_eq!(eval("(float? 3.14)"), Value::bool(true));
    assert_eq!(eval("(float? 42)"), Value::bool(false));
}

#[test]
fn test_vector_predicate() {
    assert_eq!(eval("(vector? [1 2 3])"), Value::bool(true));
    assert_eq!(eval("(vector? (list 1 2 3))"), Value::bool(false));
    assert_eq!(eval("(vector? 42)"), Value::bool(false));
}

#[test]
fn test_even_odd() {
    assert_eq!(eval("(even? 0)"), Value::bool(true));
    assert_eq!(eval("(even? 2)"), Value::bool(true));
    assert_eq!(eval("(even? 3)"), Value::bool(false));
    assert_eq!(eval("(even? -4)"), Value::bool(true));
    assert_eq!(eval("(odd? 1)"), Value::bool(true));
    assert_eq!(eval("(odd? 2)"), Value::bool(false));
    assert_eq!(eval("(odd? -3)"), Value::bool(true));
}

#[test]
fn test_zero_positive_negative() {
    assert_eq!(eval("(zero? 0)"), Value::bool(true));
    assert_eq!(eval("(zero? 0.0)"), Value::bool(true));
    assert_eq!(eval("(zero? 1)"), Value::bool(false));
    assert_eq!(eval("(positive? 1)"), Value::bool(true));
    assert_eq!(eval("(positive? 0)"), Value::bool(false));
    assert_eq!(eval("(positive? -1)"), Value::bool(false));
    assert_eq!(eval("(negative? -1)"), Value::bool(true));
    assert_eq!(eval("(negative? 0)"), Value::bool(false));
    assert_eq!(eval("(negative? 1)"), Value::bool(false));
}

#[test]
fn test_eq_identity() {
    // eq? uses structural equality in Sema (PartialEq)
    assert_eq!(eval("(eq? 1 1)"), Value::bool(true));
    assert_eq!(eval("(eq? :a :a)"), Value::bool(true));
    assert_eq!(eval("(eq? \"hello\" \"hello\")"), Value::bool(true));
    assert_eq!(eval("(eq? #t #t)"), Value::bool(true));
    assert_eq!(eval("(eq? nil nil)"), Value::bool(true));
    assert_eq!(eval("(eq? 1 2)"), Value::bool(false));
    assert_eq!(eval("(eq? 1 1.0)"), Value::bool(false)); // different types
                                                         // Lists: structural equality
    assert_eq!(eval("(eq? (list 1 2) (list 1 2))"), Value::bool(true));
}

#[test]
fn test_string_ref() {
    assert_eq!(eval(r#"(string-ref "hello" 0)"#), Value::char('h'));
    assert_eq!(eval(r#"(string-ref "hello" 4)"#), Value::char('o'));
}

#[test]
fn test_string_ref_out_of_bounds_includes_length() {
    let err = eval_err(r#"(string-ref "hello" 10)"#).to_string();
    assert!(
        err.contains("out of bounds"),
        "expected 'out of bounds', got: {err}"
    );
    assert!(
        err.contains("string length 5"),
        "expected string length in error, got: {err}"
    );
}

#[test]
fn test_substring() {
    assert_eq!(eval(r#"(substring "hello" 1 3)"#), Value::string("el"));
    assert_eq!(eval(r#"(substring "hello" 0 5)"#), Value::string("hello"));
    assert_eq!(eval(r#"(substring "hello" 3)"#), Value::string("lo"));
}

#[test]
fn test_string_trim() {
    assert_eq!(eval(r#"(string/trim "  hello  ")"#), Value::string("hello"));
    assert_eq!(eval(r#"(string/trim "\thello\n")"#), Value::string("hello"));
    assert_eq!(eval(r#"(string/trim "hello")"#), Value::string("hello"));
}

#[test]
fn test_string_starts_ends_with() {
    assert_eq!(
        eval(r#"(string/starts-with? "hello" "hel")"#),
        Value::bool(true)
    );
    assert_eq!(
        eval(r#"(string/starts-with? "hello" "ell")"#),
        Value::bool(false)
    );
    assert_eq!(
        eval(r#"(string/ends-with? "hello" "llo")"#),
        Value::bool(true)
    );
    assert_eq!(
        eval(r#"(string/ends-with? "hello" "hel")"#),
        Value::bool(false)
    );
}

#[test]
fn test_string_upper_lower() {
    assert_eq!(eval(r#"(string/upper "hello")"#), Value::string("HELLO"));
    assert_eq!(eval(r#"(string/lower "HELLO")"#), Value::string("hello"));
    assert_eq!(eval(r#"(string/upper "")"#), Value::string(""));
}

#[test]
fn test_string_replace() {
    assert_eq!(
        eval(r#"(string/replace "hello world" "world" "sema")"#),
        Value::string("hello sema")
    );
    assert_eq!(
        eval(r#"(string/replace "aaa" "a" "bb")"#),
        Value::string("bbbbbb")
    );
}

#[test]
fn test_string_join() {
    assert_eq!(
        eval(r#"(string/join (list "a" "b" "c") ", ")"#),
        Value::string("a, b, c")
    );
    assert_eq!(
        eval(r#"(string/join (list "one") "-")"#),
        Value::string("one")
    );
    assert_eq!(eval(r#"(string/join (list) ", ")"#), Value::string(""));
}

#[test]
fn test_str_concat() {
    assert_eq!(eval(r#"(str "a" 1 :b)"#), Value::string("a1:b"));
    assert_eq!(eval(r#"(str)"#), Value::string(""));
    assert_eq!(eval(r#"(str "hello")"#), Value::string("hello"));
}

#[test]
fn test_string_append_coercion() {
    // string-append coerces non-strings
    assert_eq!(
        eval(r#"(string-append "val=" 42)"#),
        Value::string("val=42")
    );
}

#[test]
fn test_hash_map() {
    assert_eq!(eval("(get (hash-map :x 1 :y 2) :x)"), Value::int(1));
    assert_eq!(eval("(get (hash-map :x 1 :y 2) :y)"), Value::int(2));
}

#[test]
fn test_dissoc() {
    assert_eq!(eval("(get (dissoc {:a 1 :b 2 :c 3} :b) :b)"), Value::nil());
    assert_eq!(eval("(count (dissoc {:a 1 :b 2 :c 3} :b))"), Value::int(2));
    // Dissoc multiple keys
    assert_eq!(
        eval("(count (dissoc {:a 1 :b 2 :c 3} :a :c))"),
        Value::int(1)
    );
}

#[test]
fn test_vals() {
    // vals returns values; BTreeMap is sorted by key
    let result = eval_to_string("(sort (vals {:a 1 :b 2 :c 3}))");
    assert_eq!(result, "(1 2 3)");
}

#[test]
fn test_merge() {
    assert_eq!(eval("(get (merge {:a 1} {:b 2} {:c 3}) :b)"), Value::int(2));
    // Later maps override earlier
    assert_eq!(eval("(get (merge {:a 1} {:a 99}) :a)"), Value::int(99));
    // Empty merge
    assert_eq!(eval_to_string("(merge)"), "{}");
}

#[test]
fn test_contains() {
    assert_eq!(eval("(contains? {:a 1 :b 2} :a)"), Value::bool(true));
    assert_eq!(eval("(contains? {:a 1 :b 2} :c)"), Value::bool(false));
}

#[test]
fn test_count() {
    assert_eq!(eval("(count {:a 1 :b 2})"), Value::int(2));
    assert_eq!(eval("(count (list 1 2 3))"), Value::int(3));
    assert_eq!(eval("(count [1 2 3 4])"), Value::int(4));
    assert_eq!(eval(r#"(count "hello")"#), Value::int(5));
    assert_eq!(eval("(count nil)"), Value::int(0));
}

#[test]
fn test_empty() {
    assert_eq!(eval("(empty? (list))"), Value::bool(true));
    assert_eq!(eval("(empty? (list 1))"), Value::bool(false));
    assert_eq!(eval("(empty? {})"), Value::bool(true));
    assert_eq!(eval("(empty? {:a 1})"), Value::bool(false));
    assert_eq!(eval("(empty? [])"), Value::bool(true));
    assert_eq!(eval(r#"(empty? "")"#), Value::bool(true));
    assert_eq!(eval("(empty? nil)"), Value::bool(true));
}

#[test]
fn test_get_with_default() {
    assert_eq!(eval("(get {:a 1} :b 42)"), Value::int(42));
    assert_eq!(eval("(get {:a 1} :a 42)"), Value::int(1));
    assert_eq!(eval("(get {:a 1} :missing)"), Value::nil());
}

#[test]
fn test_keyword_as_fn_missing_key() {
    // Keyword lookup on map returns nil for missing key
    assert_eq!(eval("(:missing {:a 1})"), Value::nil());
}

#[test]
fn test_round() {
    // Exactness-preserving (R7RS): a float argument rounds to a float.
    assert_eq!(eval("(round 3.4)"), Value::float(3.0));
    assert_eq!(eval("(round 3.5)"), Value::float(4.0));
    assert_eq!(eval("(round -1.5)"), Value::float(-2.0));
    assert_eq!(eval("(round 5)"), Value::int(5)); // int passthrough
}

#[test]
fn test_sqrt() {
    // sqrt of a perfect square is exact (R7RS): 16 => 4, not 4.0.
    assert_eq!(eval("(sqrt 16)"), Value::int(4));
    assert_float_eq("(sqrt 2.0)", 2.0_f64.sqrt());
}

#[test]
fn test_pow() {
    assert_eq!(eval("(pow 2 10)"), Value::int(1024));
    assert_eq!(eval("(pow 3 0)"), Value::int(1));
    assert_float_eq("(pow 2.0 0.5)", 2.0_f64.powf(0.5));
}

#[test]
fn test_log() {
    assert_eq!(eval("(log 1)"), Value::float(0.0));
    // log(e) ≈ 1.0
    let result = eval("(log e)");
    if let Some(f) = result.as_float() {
        assert!((f - 1.0).abs() < 1e-10);
    } else {
        panic!("expected float");
    }
}

#[test]
fn test_trig() {
    assert_float_eq("(sin 0)", 0.0);
    assert_float_eq("(cos 0)", 1.0);
    // sin(pi/2) ≈ 1.0
    let result = eval("(sin (/ pi 2))");
    if let Some(f) = result.as_float() {
        assert!((f - 1.0).abs() < 1e-10);
    } else {
        panic!("expected float");
    }
}

#[test]
fn test_pi_and_e_constants() {
    if let Some(p) = eval("pi").as_float() {
        assert!((p - std::f64::consts::PI).abs() < 1e-15);
    } else {
        panic!("expected float");
    }
    if let Some(e) = eval("e").as_float() {
        assert!((e - std::f64::consts::E).abs() < 1e-15);
    } else {
        panic!("expected float");
    }
}

#[test]
fn test_int_float_conversion() {
    assert_eq!(eval("(int 3.7)"), Value::int(3));
    assert_eq!(eval("(int 42)"), Value::int(42));
    assert_eq!(eval(r#"(int "99")"#), Value::int(99));
    assert_eq!(eval("(float 42)"), Value::float(42.0));
    assert_eq!(eval("(float 3.14)"), Value::float(3.14));
    assert_eq!(eval(r#"(float "2.5")"#), Value::float(2.5));
}

#[test]
fn test_division_by_zero() {
    let err = eval_err("(/ 1 0)");
    assert!(matches!(err.inner(), SemaError::Eval(_)));
    let err = eval_err("(mod 5 0)");
    assert!(matches!(err.inner(), SemaError::Eval(_)));
}

#[test]
fn test_arithmetic_identity() {
    // (+) => 0, (*) => 1 (identity elements)
    assert_eq!(eval("(+)"), Value::int(0));
    assert_eq!(eval("(*)"), Value::int(1));
}

#[test]
fn test_unary_minus() {
    assert_eq!(eval("(- 5)"), Value::int(-5));
    assert_eq!(eval("(- -3)"), Value::int(3));
    assert_eq!(eval("(- 2.5)"), Value::float(-2.5));
}

#[test]
fn test_mixed_int_float_arithmetic() {
    assert_eq!(eval("(+ 1 2.0)"), Value::float(3.0));
    assert_eq!(eval("(* 3 1.5)"), Value::float(4.5));
    assert_eq!(eval("(- 10 2.5)"), Value::float(7.5));
    assert_eq!(eval("(/ 10 2)"), Value::int(5)); // integer division when exact
    assert_eq!(eval("(/ 7 2)"), eval("7/2")); // exact/exact → exact rational (R7RS)
    assert_eq!(eval("(/ 7.0 2)"), Value::float(3.5)); // inexact operand → float
}

#[test]
fn test_chained_comparison() {
    assert_eq!(eval("(< 1 2 3 4)"), Value::bool(true));
    assert_eq!(eval("(< 1 2 2 4)"), Value::bool(false));
    assert_eq!(eval("(>= 4 3 2 1)"), Value::bool(true));
    assert_eq!(eval("(>= 4 3 3 1)"), Value::bool(true));
    assert_eq!(eval("(= 5 5 5 5)"), Value::bool(true));
    assert_eq!(eval("(= 5 5 4 5)"), Value::bool(false));
}

#[test]
fn test_arity_errors() {
    assert_arity_error("(car)");
    assert_arity_error("(car 1 2)");
    assert_arity_error("(not)");
    assert_arity_error("(string-length)");
}

#[test]
#[ignore = "arity call-form note deferred — separate from stack traces"]
fn test_arity_error_shows_call_form() {
    // Lambda arity error should include the call form in a note
    let err = eval_err("(define (f x) x) (f 1 2 3)");
    assert!(err.note().is_some(), "arity error should have a note");
    let note = err.note().unwrap();
    assert!(note.contains("in:"), "note should contain 'in:': {note}");
    assert!(
        note.contains("f"),
        "note should contain function name: {note}"
    );

    // Native fn arity error should also include the call form
    let err = eval_err("(car 1 2)");
    assert!(
        err.note().is_some(),
        "native arity error should have a note"
    );
    let note = err.note().unwrap();
    assert!(note.contains("in:"), "note should contain 'in:': {note}");
    assert!(
        note.contains("car"),
        "note should contain function name: {note}"
    );
}

#[test]
fn test_type_errors() {
    let err = eval_err(r#"(+ 1 "hello")"#);
    assert!(matches!(err.inner(), SemaError::Type { .. }));
    let err = eval_err("(car 42)");
    assert!(matches!(err.inner(), SemaError::Type { .. }));
    // NOTE: `(< "a" "b")` supports lexicographic string comparison, returning #t.
    // That VM capability is the canonical behavior now, so this is no longer a
    // type-error case.
    assert_eq!(eval(r#"(< "a" "b")"#), Value::bool(true));
}

#[test]
fn test_unbound_variable() {
    let err = eval_err("no-such-var");
    assert!(matches!(err.inner(), SemaError::Unbound(_)));
}

#[test]
fn test_truthiness() {
    // Only nil and #f are falsy
    assert_eq!(eval("(if nil 1 2)"), Value::int(2));
    assert_eq!(eval("(if #f 1 2)"), Value::int(2));
    // Everything else is truthy, including 0, empty string, empty list
    assert_eq!(eval("(if 0 1 2)"), Value::int(1));
    assert_eq!(eval(r#"(if "" 1 2)"#), Value::int(1));
    assert_eq!(eval("(if (list) 1 2)"), Value::int(1));
    assert_eq!(eval("(if :foo 1 2)"), Value::int(1));
}

#[test]
fn test_and_or_empty() {
    assert_eq!(eval("(and)"), Value::bool(true));
    assert_eq!(eval("(or)"), Value::bool(false));
}

#[test]
fn test_begin_empty() {
    assert_eq!(eval("(begin)"), Value::nil());
}

#[test]
fn test_if_two_branch() {
    // if with no else returns nil
    assert_eq!(eval("(if #f 42)"), Value::nil());
    assert_eq!(eval("(if #t 42)"), Value::int(42));
}

#[test]
fn test_let_empty_bindings() {
    assert_eq!(eval("(let () 42)"), Value::int(42));
    assert_eq!(eval("(let* () 42)"), Value::int(42));
}

#[test]
fn test_fn_alias() {
    // fn is an alias for lambda
    assert_eq!(eval("((fn (x) (* x x)) 5)"), Value::int(25));
    assert_eq!(
        eval("(begin (define square (fn (x) (* x x))) (square 7))"),
        Value::int(49)
    );
}

#[test]
fn test_do_loop() {
    // do is now a proper Scheme iteration form
    // Sum 1..10
    assert_eq!(
        eval("(do ((i 0 (+ i 1)) (sum 0 (+ sum i))) ((= i 10) sum))"),
        Value::int(45)
    );
    // begin still works for sequencing
    assert_eq!(eval("(begin 1 2 3)"), Value::int(3));
}

#[test]
fn test_unquote_splicing() {
    assert_eq!(
        eval_to_string("(begin (define xs (list 1 2 3)) `(a ,@xs b))"),
        "(a 1 2 3 b)"
    );
    assert_eq!(eval_to_string("`(,@(list 1 2) ,@(list 3 4))"), "(1 2 3 4)");
    // Splicing empty list
    assert_eq!(eval_to_string("`(a ,@(list) b)"), "(a b)");
}

#[test]
fn test_nested_closures() {
    // Closure over multiple layers of scope
    assert_eq!(
        eval("(begin (define (make-counter) (define n 0) (lambda () (set! n (+ n 1)) n)) (define c (make-counter)) (c) (c) (c))"),
        Value::int(3)
    );
}

#[test]
fn test_closure_independence() {
    // Two closures over the same factory don't share state
    assert_eq!(
        eval("(begin (define (make-counter) (define n 0) (lambda () (set! n (+ n 1)) n)) (define a (make-counter)) (define b (make-counter)) (a) (a) (a) (b) (b) (list (a) (b)))"),
        eval("(list 4 3)")
    );
}

#[test]
fn test_internal_define() {
    assert_eq!(
        eval("(begin (define (f x) (define y (* x 2)) (+ y 1)) (f 5))"),
        Value::int(11)
    );
}

#[test]
fn test_closure_captures_environment() {
    // Closure captures variables, not values
    assert_eq!(
        eval("(begin (define x 1) (define f (lambda () x)) (set! x 2) (f))"),
        Value::int(2)
    );
}

#[test]
fn test_tco_deep_recursion() {
    // If TCO is broken, this will stack overflow
    assert_eq!(
        eval(
            "(begin (define (sum n acc) (if (= n 0) acc (sum (- n 1) (+ n acc)))) (sum 100000 0))"
        ),
        Value::int(5000050000)
    );
}

#[test]
fn test_tco_mutual_recursion() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define (is-even? n) (if (= n 0) #t (is-odd? (- n 1))))
              (define (is-odd? n) (if (= n 0) #f (is-even? (- n 1))))
              (is-even? 10000))
        "#
        ),
        Value::bool(true)
    );
}

#[test]
fn test_tco_named_let() {
    // Deep named-let loop
    assert_eq!(
        eval("(let loop ((i 100000) (acc 0)) (if (= i 0) acc (loop (- i 1) (+ acc i))))"),
        Value::int(5000050000)
    );
}

#[test]
fn test_for_each() {
    // for-each returns nil
    assert_eq!(eval("(for-each (lambda (x) x) (list 1 2 3))"), Value::nil());
    // for-each on empty list
    assert_eq!(eval("(for-each (lambda (x) x) (list))"), Value::nil());
}

#[test]
fn test_map_empty_list() {
    assert_eq!(eval_to_string("(map + (list))"), "()");
    assert_eq!(eval_to_string("(filter even? (list))"), "()");
    assert_eq!(eval("(foldl + 0 (list))"), Value::int(0));
}

#[test]
fn test_cons_behavior() {
    // cons onto a list
    assert_eq!(eval_to_string("(cons 1 (list 2 3))"), "(1 2 3)");
    // cons onto empty list
    assert_eq!(eval_to_string("(cons 1 (list))"), "(1)");
    // cons of two atoms creates a two-element list in Sema
    assert_eq!(eval_to_string("(cons 1 2)"), "(1 2)");
}

#[test]
fn test_apply_with_prefix_args() {
    assert_eq!(eval("(apply + 1 2 (list 3 4))"), Value::int(10));
    assert_eq!(eval("(apply + (list))"), Value::int(0));
    assert_eq!(eval("(apply list 1 2 (list 3))"), eval("(list 1 2 3)"));
}

#[test]
fn test_nested_list_ops() {
    // map over map results
    assert_eq!(
        eval_to_string("(map (lambda (x) (+ x 1)) (map (lambda (x) (* x 2)) (list 1 2 3)))"),
        "(3 5 7)"
    );
}

#[test]
fn test_format_directives() {
    // ~a: display without quotes
    assert_eq!(eval(r#"(format "~a" "hello")"#), Value::string("hello"));
    // ~s: write with quotes
    assert_eq!(eval(r#"(format "~s" "hello")"#), Value::string("\"hello\""));
    // ~%: newline
    assert_eq!(eval(r#"(format "a~%b")"#), Value::string("a\nb"));
    // ~~: literal tilde
    assert_eq!(eval(r#"(format "~~")"#), Value::string("~"));
}

#[test]
fn test_vector_basics() {
    assert_eq!(eval("(vector? [1 2 3])"), Value::bool(true));
    assert_eq!(eval("(length [1 2 3])"), Value::int(3));
    assert_eq!(eval("(count [])"), Value::int(0));
    assert_eq!(eval("(empty? [])"), Value::bool(true));
    assert_eq!(eval("(empty? [1])"), Value::bool(false));
}

#[test]
fn test_cond_no_match_no_else() {
    assert_eq!(eval("(cond (#f 1) (#f 2))"), Value::nil());
}

#[test]
fn test_cond_first_match_wins() {
    assert_eq!(eval("(cond (#t 1) (#t 2) (else 3))"), Value::int(1));
}

#[test]
fn test_error_builtin() {
    let err = eval_err(r#"(error "custom error message")"#);
    assert!(err.to_string().contains("custom error message"));
}

#[test]
fn test_try_catch_returns_value_on_success() {
    assert_eq!(eval("(try (+ 1 2) (catch e 0))"), Value::int(3));
}

#[test]
fn test_try_catch_error_info_map() {
    // Caught error is a map with :type and :message
    assert_eq!(
        eval(r#"(try (error "boom") (catch e (:type e)))"#),
        Value::keyword("eval")
    );
    assert_eq!(
        eval(r#"(try (error "boom") (catch e (:message e)))"#),
        Value::string("boom")
    );
}

#[test]
fn test_try_catch_division_by_zero() {
    assert_eq!(
        eval(r#"(try (/ 1 0) (catch e (:type e)))"#),
        Value::keyword("eval")
    );
}

#[test]
fn test_negative_numbers() {
    assert_eq!(eval("-42"), Value::int(-42));
    assert_eq!(eval("-3.14"), Value::float(-3.14));
}

#[test]
fn test_string_escapes() {
    assert_eq!(eval(r#"(string-length "\n")"#), Value::int(1));
    assert_eq!(eval(r#"(string-length "\t")"#), Value::int(1));
    assert_eq!(eval(r#"(string-length "\\")"#), Value::int(1));
    assert_eq!(eval(r#"(string-length "\"")"#), Value::int(1));
}

#[test]
fn test_string_hex_escape_r7rs() {
    // \x<hex>; R7RS-style
    assert_eq!(eval(r#""\x41;""#), Value::string("A"));
    assert_eq!(eval(r#""\x1B;""#), Value::string("\x1B"));
    assert_eq!(eval(r#""\x3BB;""#), Value::string("λ"));
    assert_eq!(eval(r#"(string-length "\x41;")"#), Value::int(1));
    // string-length counts characters; U+1F600 is 1 character
    assert_eq!(eval(r#"(string-length "\x1F600;")"#), Value::int(1));
}

#[test]
fn test_string_u_escape() {
    assert_eq!(eval(r#""\u0041""#), Value::string("A"));
    assert_eq!(eval(r#""\u03BB""#), Value::string("λ"));
    assert_eq!(eval(r#"(string-length "\u0041")"#), Value::int(1));
}

#[test]
fn test_string_big_u_escape() {
    assert_eq!(eval(r#""\U00000041""#), Value::string("A"));
    assert_eq!(eval(r#""\U0001F600""#), Value::string("😀"));
    // string-length counts characters; U+1F600 is 1 character
    assert_eq!(eval(r#"(string-length "\U0001F600")"#), Value::int(1));
}

#[test]
fn test_string_null_escape() {
    assert_eq!(eval(r#"(string-length "\0")"#), Value::int(1));
}

#[test]
fn test_string_mixed_escape_types() {
    assert_eq!(eval(r#""\x48;\u0069""#), Value::string("Hi"));
    assert_eq!(
        eval(r#"(string-append "\x48;" "\u0069")"#),
        Value::string("Hi")
    );
}

#[test]
fn test_string_ansi_escape_codes() {
    // Real-world: ANSI color escape sequences
    // "\x1B;[31m" = ESC [ 3 1 m = 5 bytes
    let result = eval(r#"(string-length "\x1B;[31m")"#);
    assert_eq!(result, Value::int(5));
}

#[test]
fn test_begin_returns_last() {
    assert_eq!(eval("(begin (+ 1 2) (+ 3 4) (+ 5 6))"), Value::int(11));
}

#[test]
fn test_lambda_multi_body() {
    assert_eq!(
        eval("((lambda (x) (+ x 1) (+ x 2) (+ x 3)) 10)"),
        Value::int(13)
    );
}

#[test]
fn test_compose_higher_order() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define (compose f g) (lambda (x) (f (g x))))
              (define (add1 x) (+ x 1))
              (define (double x) (* x 2))
              (define add1-then-double (compose double add1))
              (define double-then-add1 (compose add1 double))
              (list (add1-then-double 3) (double-then-add1 3)))
        "#
        ),
        eval("(list 8 7)")
    );
}

#[test]
fn test_partial_application_pattern() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define (partial f . args)
                (lambda (. rest) (apply f (append args rest))))
              (define add5 (partial + 5))
              (add5 3))
        "#
        ),
        Value::int(8)
    );
}

#[test]
fn test_ackermann() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define (ack m n)
                (cond
                  ((= m 0) (+ n 1))
                  ((= n 0) (ack (- m 1) 1))
                  (else (ack (- m 1) (ack m (- n 1))))))
              (ack 3 4))
        "#
        ),
        Value::int(125)
    );
}

#[test]
fn test_flatten_deep() {
    // flatten is one-level deep in Sema
    assert_eq!(
        eval_to_string("(flatten (list (list 1 (list 2 3)) (list 4) 5))"),
        "(1 (2 3) 4 5)"
    );
    // Full flat for already-flat nested lists
    assert_eq!(
        eval_to_string("(flatten (list (list 1 2) (list 3 4) (list 5)))"),
        "(1 2 3 4 5)"
    );
}

#[test]
fn test_defmacro_with_quasiquote() {
    assert_eq!(
        eval(
            r#"
            (begin
              (defmacro swap! (a b)
                `(let ((tmp ,a)) (set! ,a ,b) (set! ,b tmp)))
              (define x 1)
              (define y 2)
              (swap! x y)
              (list x y))
        "#
        ),
        eval("(list 2 1)")
    );
}

#[test]
fn test_defmacro_unless_custom() {
    assert_eq!(
        eval(
            r#"
            (begin
              (defmacro my-unless (test body)
                (list 'if test nil body))
              (my-unless #f 42))
        "#
        ),
        Value::int(42)
    );
}

// New stdlib functions — tests for features added in current dev branch

#[test]
fn test_list_index_of() {
    assert_eq!(eval("(list/index-of (list 10 20 30) 20)"), Value::int(1));
    assert_eq!(eval("(list/index-of (list 10 20 30) 10)"), Value::int(0));
    assert_eq!(eval("(list/index-of (list 10 20 30) 99)"), Value::nil());
    assert_eq!(eval("(list/index-of (list) 1)"), Value::nil());
}

#[test]
fn test_list_unique() {
    assert_eq!(
        eval_to_string("(list/unique (list 1 2 3 2 1 4))"),
        "(1 2 3 4)"
    );
    assert_eq!(eval_to_string("(list/unique (list))"), "()");
    assert_eq!(
        eval_to_string("(list/unique (list :a :b :a :c :b))"),
        "(:a :b :c)"
    );
}

#[test]
fn test_list_group_by() {
    let result = eval(r#"(list/group-by even? (list 1 2 3 4 5 6))"#);
    if let Some(m) = result.as_map_rc() {
        assert_eq!(
            m.get(&Value::bool(true)).cloned(),
            Some(eval("(list 2 4 6)"))
        );
        assert_eq!(
            m.get(&Value::bool(false)).cloned(),
            Some(eval("(list 1 3 5)"))
        );
    } else {
        panic!("expected map, got {result}");
    }
}

#[test]
fn test_list_interleave() {
    assert_eq!(
        eval_to_string("(list/interleave (list 1 2 3) (list :a :b :c))"),
        "(1 :a 2 :b 3 :c)"
    );
    // Shortest wins
    assert_eq!(
        eval_to_string("(list/interleave (list 1 2) (list :a :b :c))"),
        "(1 :a 2 :b)"
    );
}

#[test]
fn test_list_chunk() {
    assert_eq!(
        eval_to_string("(list/chunk 2 (list 1 2 3 4 5))"),
        "((1 2) (3 4) (5))"
    );
    assert_eq!(
        eval_to_string("(list/chunk 3 (list 1 2 3 4 5 6))"),
        "((1 2 3) (4 5 6))"
    );
    assert_eq!(eval_to_string("(list/chunk 2 (list))"), "()");
}

#[test]
fn test_map_entries() {
    // BTreeMap is sorted by key, so entries are in key order
    assert_eq!(
        eval_to_string("(map/entries {:a 1 :b 2})"),
        "((:a 1) (:b 2))"
    );
    assert_eq!(eval_to_string("(map/entries {})"), "()");
}

#[test]
fn test_map_map_vals() {
    assert_eq!(
        eval("(get (map/map-vals (lambda (v) (* v 2)) {:a 1 :b 2 :c 3}) :b)"),
        Value::int(4)
    );
    assert_eq!(
        eval("(get (map/map-vals (lambda (v) (+ v 10)) {:x 5}) :x)"),
        Value::int(15)
    );
}

#[test]
fn test_map_filter() {
    assert_eq!(
        eval("(count (map/filter (lambda (k v) (> v 1)) {:a 1 :b 2 :c 3}))"),
        Value::int(2)
    );
    assert_eq!(
        eval("(get (map/filter (lambda (k v) (> v 1)) {:a 1 :b 2 :c 3}) :a)"),
        Value::nil()
    );
}

#[test]
fn test_map_select_keys() {
    assert_eq!(
        eval("(count (map/select-keys {:a 1 :b 2 :c 3} (list :a :c)))"),
        Value::int(2)
    );
    assert_eq!(
        eval("(get (map/select-keys {:a 1 :b 2 :c 3} (list :a :c)) :b)"),
        Value::nil()
    );
    assert_eq!(
        eval("(get (map/select-keys {:a 1 :b 2 :c 3} (list :a :c)) :a)"),
        Value::int(1)
    );
}

#[test]
fn test_map_update() {
    assert_eq!(
        eval("(get (map/update {:a 1 :b 2} :a (lambda (v) (+ v 10))) :a)"),
        Value::int(11)
    );
    // Missing key → fn gets nil
    assert_eq!(
        eval(r#"(get (map/update {:a 1} :b (lambda (v) "default")) :b)"#),
        Value::string("default")
    );
}

#[test]
fn test_string_index_of() {
    assert_eq!(
        eval(r#"(string/index-of "hello world" "world")"#),
        Value::int(6)
    );
    assert_eq!(eval(r#"(string/index-of "hello" "xyz")"#), Value::nil());
    assert_eq!(eval(r#"(string/index-of "abcabc" "bc")"#), Value::int(1));
}

#[test]
fn test_string_chars() {
    assert_eq!(
        eval_to_string(r#"(string/chars "abc")"#),
        r#"(#\a #\b #\c)"#
    );
    assert_eq!(eval_to_string(r#"(string/chars "")"#), "()");
}

#[test]
fn test_string_repeat() {
    assert_eq!(eval(r#"(string/repeat "ab" 3)"#), Value::string("ababab"));
    assert_eq!(eval(r#"(string/repeat "x" 0)"#), Value::string(""));
    assert_eq!(eval(r#"(string/repeat "" 5)"#), Value::string(""));
}

#[test]
fn test_string_pad_left() {
    assert_eq!(eval(r#"(string/pad-left "42" 5)"#), Value::string("   42"));
    assert_eq!(
        eval(r#"(string/pad-left "42" 5 "0")"#),
        Value::string("00042")
    );
    assert_eq!(
        eval(r#"(string/pad-left "hello" 3)"#),
        Value::string("hello")
    ); // longer than width
}

#[test]
fn test_string_pad_right() {
    assert_eq!(eval(r#"(string/pad-right "42" 5)"#), Value::string("42   "));
    assert_eq!(
        eval(r#"(string/pad-right "42" 5 ".")"#),
        Value::string("42...")
    );
    assert_eq!(
        eval(r#"(string/pad-right "hello" 3)"#),
        Value::string("hello")
    );
}

#[test]
fn test_math_quotient_remainder() {
    assert_eq!(eval("(math/quotient 13 4)"), Value::int(3));
    assert_eq!(eval("(math/quotient -13 4)"), Value::int(-3));
    assert_eq!(eval("(math/remainder 13 4)"), Value::int(1));
    assert_eq!(eval("(math/remainder -13 4)"), Value::int(-1));
    // Division by zero → Eval error
    assert!(
        matches!(eval_err("(math/quotient 5 0)").inner(), SemaError::Eval(msg) if msg.contains("division by zero"))
    );
    assert!(
        matches!(eval_err("(math/remainder 5 0)").inner(), SemaError::Eval(msg) if msg.contains("division by zero"))
    );
}

#[test]
fn test_math_gcd_lcm() {
    assert_eq!(eval("(math/gcd 12 8)"), Value::int(4));
    assert_eq!(eval("(math/gcd 7 13)"), Value::int(1));
    assert_eq!(eval("(math/gcd -12 8)"), Value::int(4));
    assert_eq!(eval("(math/gcd 0 5)"), Value::int(5));
    assert_eq!(eval("(math/lcm 4 6)"), Value::int(12));
    assert_eq!(eval("(math/lcm 3 5)"), Value::int(15));
    assert_eq!(eval("(math/lcm 0 0)"), Value::int(0));
}

#[test]
fn test_math_trig_extended() {
    // tan(0) = 0
    assert_float_eq("(math/tan 0)", 0.0);
    // asin(0) = 0, acos(1) = 0, atan(0) = 0
    assert_float_eq("(math/asin 0)", 0.0);
    assert_float_eq("(math/acos 1)", 0.0);
    assert_float_eq("(math/atan 0)", 0.0);
    // atan2(0, 1) = 0, atan2(1, 0) = pi/2
    assert_float_eq("(math/atan2 0 1)", 0.0);
    if let Some(f) = eval("(math/atan2 1 0)").as_float() {
        assert!((f - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
    } else {
        panic!("expected float");
    }
}

#[test]
fn test_math_exp_log() {
    // exp(0) = 1
    assert_float_eq("(math/exp 0)", 1.0);
    // exp(1) ≈ e
    if let Some(f) = eval("(math/exp 1)").as_float() {
        assert!((f - std::f64::consts::E).abs() < 1e-10);
    } else {
        panic!("expected float");
    }
    // log10(100) = 2 (use approximate comparison for transcendental functions)
    let v = eval("(math/log10 100)").as_float().unwrap();
    assert!((v - 2.0).abs() < 1e-10, "log10(100) = {v}");
    // log2(8) = 3
    let v = eval("(math/log2 8)").as_float().unwrap();
    assert!((v - 3.0).abs() < 1e-10, "log2(8) = {v}");
}

#[test]
fn test_math_random() {
    // math/random returns a float in [0, 1)
    if let Some(f) = eval("(math/random)").as_float() {
        assert!((0.0..1.0).contains(&f));
    } else {
        panic!("expected float");
    }
    // math/random-int returns int in range
    if let Some(n) = eval("(math/random-int 1 10)").as_int() {
        assert!((1..=10).contains(&n));
    } else {
        panic!("expected int");
    }
}

#[test]
fn test_math_clamp() {
    assert_eq!(eval("(math/clamp 5 1 10)"), Value::int(5));
    assert_eq!(eval("(math/clamp -5 1 10)"), Value::int(1));
    assert_eq!(eval("(math/clamp 15 1 10)"), Value::int(10));
    assert_eq!(eval("(math/clamp 0.5 0.0 1.0)"), Value::float(0.5));
    assert_eq!(eval("(math/clamp -1.0 0.0 1.0)"), Value::float(0.0));
}

#[test]
fn test_math_sign() {
    assert_eq!(eval("(math/sign 42)"), Value::int(1));
    assert_eq!(eval("(math/sign -5)"), Value::int(-1));
    assert_eq!(eval("(math/sign 0)"), Value::int(0));
    assert_eq!(eval("(math/sign 3.14)"), Value::int(1));
    assert_eq!(eval("(math/sign -0.5)"), Value::int(-1));
}

#[test]
fn test_bitwise_ops() {
    assert_eq!(eval("(bit/and 12 10)"), Value::int(8)); // 1100 & 1010 = 1000
    assert_eq!(eval("(bit/or 12 10)"), Value::int(14)); // 1100 | 1010 = 1110
    assert_eq!(eval("(bit/xor 12 10)"), Value::int(6)); // 1100 ^ 1010 = 0110
    assert_eq!(eval("(bit/not 0)"), Value::int(-1));
    assert_eq!(eval("(bit/shift-left 1 4)"), Value::int(16));
    assert_eq!(eval("(bit/shift-right 16 4)"), Value::int(1));
}

#[test]
fn test_bitwise_edge_cases() {
    assert_eq!(eval("(bit/and 0 255)"), Value::int(0));
    assert_eq!(eval("(bit/or 0 0)"), Value::int(0));
    assert_eq!(eval("(bit/xor 42 42)"), Value::int(0)); // x ^ x = 0
    assert_eq!(eval("(bit/xor 42 0)"), Value::int(42)); // x ^ 0 = x
    assert_eq!(eval("(bit/shift-left 1 0)"), Value::int(1)); // no shift
}

#[test]
fn test_sys_cwd() {
    let result = eval("(sys/cwd)");
    assert!(result.is_string());
    // Should be a non-empty string
    if let Some(s) = result.as_str() {
        assert!(!s.is_empty());
    }
}

#[test]
fn test_sys_platform() {
    let result = eval("(sys/platform)");
    if let Some(s) = result.as_str() {
        assert!(["macos", "linux", "windows", "unknown"].contains(&s));
    } else {
        panic!("expected string");
    }
}

#[test]
fn test_sys_args() {
    // sys/args should return a list. Backed by `std::env::args()`, which is
    // guaranteed to include at least argv[0] (the test binary's path) on
    // every supported platform, so we can additionally assert non-empty.
    let result = eval("(sys/args)");
    assert!(result.is_list());
    let items = result.as_list().expect("sys/args should be a list");
    assert!(
        !items.is_empty(),
        "sys/args should contain at least argv[0]"
    );
}

#[test]
fn test_sys_env_all() {
    // sys/env-all should return a map
    let result = eval("(sys/env-all)");
    assert!(result.is_map());
    // Should have at least PATH or HOME
    if let Some(m) = result.as_map_rc() {
        assert!(!m.is_empty());
    }
}

#[test]
fn test_file_is_file() {
    let dir = temp_path("sema-test-isfile");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(format!("{dir}/a.txt"), "hello").unwrap();

    assert_eq!(
        eval(&format!(r#"(file/is-file? "{dir}/a.txt")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/is-file? "{dir}")"#)),
        Value::bool(false)
    );
    assert_eq!(
        eval(&format!(
            r#"(file/is-file? "{}")"#,
            temp_path("nonexistent-sema-xyz")
        )),
        Value::bool(false)
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_is_symlink() {
    // Non-symlink file should return false
    let dir = temp_path("sema-test-symlink");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(format!("{dir}/a.txt"), "hello").unwrap();

    assert_eq!(
        eval(&format!(r#"(file/is-symlink? "{dir}/a.txt")"#)),
        Value::bool(false)
    );
    assert_eq!(
        eval(&format!(
            r#"(file/is-symlink? "{}")"#,
            temp_path("nonexistent-sema-xyz")
        )),
        Value::bool(false)
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_uuid_v4() {
    let result = eval("(uuid/v4)");
    if let Some(s) = result.as_str() {
        assert_eq!(s.len(), 36);
        assert_eq!(s.chars().filter(|c| *c == '-').count(), 4);
    } else {
        panic!("expected string");
    }
    // Two UUIDs should be different
    assert_ne!(eval("(uuid/v4)"), eval("(uuid/v4)"));
}

#[test]
fn test_base64_encode_decode() {
    assert_eq!(
        eval(r#"(base64/encode "hello")"#),
        Value::string("aGVsbG8=")
    );
    assert_eq!(
        eval(r#"(base64/decode "aGVsbG8=")"#),
        Value::string("hello")
    );
    assert_eq!(eval(r#"(base64/encode "")"#), Value::string(""));
    assert_eq!(eval(r#"(base64/decode "")"#), Value::string(""));
    // Roundtrip
    assert_eq!(
        eval(r#"(base64/decode (base64/encode "Sema Lisp 🎉"))"#),
        Value::string("Sema Lisp 🎉")
    );
}

#[test]
fn test_hash_sha256() {
    // Known SHA-256 of "hello"
    assert_eq!(
        eval(r#"(hash/sha256 "hello")"#),
        Value::string("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
    );
    // Empty string
    assert_eq!(
        eval(r#"(hash/sha256 "")"#),
        Value::string("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
}

#[test]
fn test_time_now() {
    let result = eval("(time/now)");
    if let Some(f) = result.as_float() {
        assert!(f > 1_700_000_000.0); // After 2023
    } else {
        panic!("expected float");
    }
}

#[test]
fn test_time_format() {
    assert_eq!(
        eval(r#"(time/format 0.0 "%Y-%m-%d")"#),
        Value::string("1970-01-01")
    );
    assert_eq!(
        eval(r#"(time/format 0.0 "%H:%M:%S")"#),
        Value::string("00:00:00")
    );
}

#[test]
fn test_time_parse() {
    let result = eval(r#"(time/parse "2024-01-15 12:30:00" "%Y-%m-%d %H:%M:%S")"#);
    if let Some(f) = result.as_float() {
        // Widened range to accommodate any timezone (+/- 14 hours = ~50400s)
        assert!(f > 1_704_900_000.0 && f < 1_706_100_000.0);
    } else {
        panic!("expected float, got {result}");
    }
}

#[test]
fn test_time_date_parts() {
    let result = eval("(time/date-parts 0.0)");
    if let Some(m) = result.as_map_rc() {
        assert_eq!(m.get(&Value::keyword("year")), Some(&Value::int(1970)));
        assert_eq!(m.get(&Value::keyword("month")), Some(&Value::int(1)));
        assert_eq!(m.get(&Value::keyword("day")), Some(&Value::int(1)));
        assert_eq!(m.get(&Value::keyword("hour")), Some(&Value::int(0)));
        assert_eq!(m.get(&Value::keyword("minute")), Some(&Value::int(0)));
        assert_eq!(m.get(&Value::keyword("second")), Some(&Value::int(0)));
        assert!(m.get(&Value::keyword("weekday")).is_some());
    } else {
        panic!("expected map");
    }
}

#[test]
fn test_time_roundtrip() {
    // Format and parse should roundtrip
    assert_eq!(
        eval(
            r#"(time/format (time/parse "2024-06-15 08:30:00" "%Y-%m-%d %H:%M:%S") "%Y-%m-%d %H:%M:%S")"#
        ),
        Value::string("2024-06-15 08:30:00")
    );
}

#[test]
fn test_csv_parse() {
    assert_eq!(
        eval_to_string(r#"(csv/parse "a,b\n1,2")"#),
        r#"(("a" "b") ("1" "2"))"#
    );
    // Single row
    assert_eq!(
        eval_to_string(r#"(csv/parse "x,y,z")"#),
        r#"(("x" "y" "z"))"#
    );
}

#[test]
fn test_csv_parse_maps() {
    let result = eval(r#"(csv/parse-maps "name,age\nAlice,30\nBob,25")"#);
    if let Some(rows) = result.as_list() {
        assert_eq!(rows.len(), 2);
        if let Some(m) = rows[0].as_map_rc() {
            assert_eq!(
                m.get(&Value::keyword("name")),
                Some(&Value::string("Alice"))
            );
            assert_eq!(m.get(&Value::keyword("age")), Some(&Value::string("30")));
        } else {
            panic!("expected map");
        }
    } else {
        panic!("expected list");
    }
}

#[test]
fn test_csv_encode() {
    let result = eval(r#"(csv/encode '(("a" "b") ("1" "2")))"#);
    if let Some(s) = result.as_str() {
        assert!(s.contains("a,b"));
        assert!(s.contains("1,2"));
    } else {
        panic!("expected string");
    }
}

// Regex operations — extended coverage

#[test]
fn test_regex_match_no_match() {
    assert_eq!(
        eval(r#"(regex/match? "\\d+" "abcdef")"#),
        Value::bool(false)
    );
    assert_eq!(eval(r#"(regex/match "\\d+" "abc")"#), Value::nil());
}

#[test]
fn test_regex_match_groups_detail() {
    let result = eval(r#"(regex/match "(\\d+)-(\\w+)" "42-hello")"#);
    if let Some(m) = result.as_map_rc() {
        assert_eq!(m.get(&Value::keyword("start")), Some(&Value::int(0)));
        assert_eq!(m.get(&Value::keyword("end")), Some(&Value::int(8)));
        if let Some(groups) = m.get(&Value::keyword("groups")).and_then(|v| v.as_list()) {
            assert_eq!(groups.len(), 2);
            assert_eq!(groups[0], Value::string("42"));
            assert_eq!(groups[1], Value::string("hello"));
        } else {
            panic!("expected groups list");
        }
    } else {
        panic!("expected map");
    }
}

#[test]
fn test_regex_find_all_empty() {
    assert_eq!(
        eval_to_string(r#"(regex/find-all "\\d+" "no digits")"#),
        "()"
    );
}

#[test]
fn test_regex_replace_all_whitespace() {
    assert_eq!(
        eval(r#"(regex/replace-all "\\s+" " " "hello   world   foo")"#),
        Value::string("hello world foo")
    );
}

#[test]
fn test_regex_split_whitespace() {
    assert_eq!(
        eval_to_string(r#"(regex/split "\\s+" "hello  world  foo")"#),
        r#"("hello" "world" "foo")"#
    );
}

// String conversion functions

#[test]
fn test_string_to_symbol() {
    assert_eq!(eval(r#"(string->symbol "hello")"#), Value::symbol("hello"));
    assert_eq!(eval(r#"(symbol? (string->symbol "x"))"#), Value::bool(true));
}

#[test]
fn test_symbol_to_string() {
    assert_eq!(eval("(symbol->string 'hello)"), Value::string("hello"));
    assert_eq!(eval(r#"(string? (symbol->string 'x))"#), Value::bool(true));
}

#[test]
fn test_string_to_keyword() {
    assert_eq!(eval(r#"(string->keyword "foo")"#), Value::keyword("foo"));
    assert_eq!(
        eval(r#"(keyword? (string->keyword "bar"))"#),
        Value::bool(true)
    );
}

#[test]
fn test_keyword_to_string() {
    assert_eq!(eval("(keyword->string :foo)"), Value::string("foo"));
    assert_eq!(
        eval(r#"(string? (keyword->string :bar))"#),
        Value::bool(true)
    );
}

#[test]
fn test_number_to_string() {
    assert_eq!(eval("(number->string 42)"), Value::string("42"));
    assert_eq!(eval("(number->string -7)"), Value::string("-7"));
    assert_eq!(eval("(number->string 3.14)"), Value::string("3.14"));
}

#[test]
fn test_string_to_number() {
    assert_eq!(eval(r#"(string->number "42")"#), Value::int(42));
    assert_eq!(eval(r#"(string->number "-7")"#), Value::int(-7));
    assert_eq!(eval(r#"(string->number "3.14")"#), Value::float(3.14));
    // Unparseable input returns #f rather than erroring (R7RS).
    assert_eq!(eval(r#"(string->number "abc")"#), Value::bool(false));
}

// JSON encode-pretty

#[test]
fn test_json_encode_pretty() {
    let result = eval(r#"(json/encode-pretty {:a 1 :b 2})"#);
    if let Some(s) = result.as_str() {
        assert!(s.contains("\"a\": 1"));
        assert!(s.contains("\"b\": 2"));
        assert!(s.contains('\n')); // pretty-printed has newlines
    } else {
        panic!("expected string");
    }
}

// Meta: gensym — additional coverage

#[test]
fn test_gensym_is_symbol_type() {
    // Verify gensym result is usable as a symbol
    assert_eq!(eval(r#"(symbol? (gensym))"#), Value::bool(true));
    assert_eq!(eval(r#"(symbol? (gensym "pfx"))"#), Value::bool(true));
}

// System: env, shell, time-ms, sleep

#[test]
fn test_env_var() {
    // PATH should exist on all platforms
    let result = eval(r#"(env "PATH")"#);
    assert!(result.is_string());
    // Non-existent var returns nil
    assert_eq!(
        eval(r#"(env "SEMA_NONEXISTENT_VAR_XYZ_123")"#),
        Value::nil()
    );
}

#[test]
#[cfg(unix)]
fn test_shell_command() {
    let result = eval(r#"(shell "echo" "hello")"#);
    if let Some(m) = result.as_map_rc() {
        if let Some(stdout) = m.get(&Value::keyword("stdout")).and_then(|v| v.as_str()) {
            assert!(stdout.trim() == "hello");
        } else {
            panic!("expected stdout");
        }
        assert_eq!(m.get(&Value::keyword("exit-code")), Some(&Value::int(0)));
    } else {
        panic!("expected map");
    }
}

#[test]
fn test_time_ms() {
    let result = eval("(time-ms)");
    if let Some(ms) = result.as_int() {
        assert!(ms > 1_700_000_000_000); // After 2023 in millis
    } else {
        panic!("expected int");
    }
}

// List functions: nth, take, drop, last, zip, sort, flatten

#[test]
fn test_nth() {
    assert_eq!(eval("(nth (list 10 20 30) 0)"), Value::int(10));
    assert_eq!(eval("(nth (list 10 20 30) 2)"), Value::int(30));
    assert_eq!(eval("(nth [10 20 30] 1)"), Value::int(20));
    // Out of bounds should error
    assert!(eval_err("(nth (list 1 2) 5)")
        .to_string()
        .contains("out of bounds"));
}

#[test]
fn test_take_and_drop() {
    assert_eq!(eval_to_string("(take 3 (list 1 2 3 4 5))"), "(1 2 3)");
    assert_eq!(eval_to_string("(take 0 (list 1 2 3))"), "()");
    assert_eq!(eval_to_string("(take 10 (list 1 2))"), "(1 2)"); // take more than available
    assert_eq!(eval_to_string("(drop 2 (list 1 2 3 4 5))"), "(3 4 5)");
    assert_eq!(eval_to_string("(drop 0 (list 1 2 3))"), "(1 2 3)");
    assert_eq!(eval_to_string("(drop 10 (list 1 2))"), "()"); // drop more than available
}

#[test]
fn test_last_extended() {
    assert_eq!(eval("(last (list 1 2 3))"), Value::int(3));
    assert_eq!(eval("(last (list 42))"), Value::int(42));
    assert_eq!(eval("(last (list))"), Value::nil());
    assert_eq!(eval("(last [10 20 30])"), Value::int(30));
}

#[test]
fn test_sort_with_comparator() {
    // Sort descending using a comparator
    assert_eq!(
        eval_to_string("(sort (list 3 1 4 1 5) (lambda (a b) (- b a)))"),
        "(5 4 3 1 1)"
    );
}

#[test]
fn test_range_with_step() {
    assert_eq!(eval_to_string("(range 0 10 2)"), "(0 2 4 6 8)");
    assert_eq!(eval_to_string("(range 10 0 -2)"), "(10 8 6 4 2)");
    // Step of zero should error
    assert!(eval_err("(range 0 10 0)").to_string().contains("step"));
}

// IO: read, read-many (parsing s-expressions from strings)

#[test]
fn test_read_sexp() {
    assert_eq!(eval(r#"(read "(+ 1 2)")"#), eval("'(+ 1 2)"));
    assert_eq!(eval(r#"(read "42")"#), Value::int(42));
    assert_eq!(eval(r#"(read ":foo")"#), Value::keyword("foo"));
}

#[test]
fn test_read_many() {
    assert_eq!(
        eval_to_string(r#"(read-many "(+ 1 2) (* 3 4)")"#),
        "((+ 1 2) (* 3 4))"
    );
}

// File operations (extended): append, delete, rename, list, mkdir,
// is-directory?, info, read-lines, write-lines, copy

#[test]
fn test_file_append_standalone() {
    let dir = temp_path("sema-test-append");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));

    eval(&format!(r#"(file/write "{dir}/a.txt" "hello")"#));
    eval(&format!(r#"(file/append "{dir}/a.txt" " world")"#));
    eval(&format!(r#"(file/append "{dir}/a.txt" "!")"#));
    assert_eq!(
        eval(&format!(r#"(file/read "{dir}/a.txt")"#)),
        Value::string("hello world!")
    );

    // Append to non-existent file creates it
    eval(&format!(r#"(file/append "{dir}/new.txt" "created")"#));
    assert_eq!(
        eval(&format!(r#"(file/read "{dir}/new.txt")"#)),
        Value::string("created")
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_delete_standalone() {
    let dir = temp_path("sema-test-delete");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));

    eval(&format!(r#"(file/write "{dir}/del.txt" "bye")"#));
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/del.txt")"#)),
        Value::bool(true)
    );
    eval(&format!(r#"(file/delete "{dir}/del.txt")"#));
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/del.txt")"#)),
        Value::bool(false)
    );

    // Deleting non-existent file should error
    let err = eval_err(&format!(r#"(file/delete "{dir}/nonexistent.txt")"#));
    assert!(err.to_string().contains("file/delete"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_rename_standalone() {
    let dir = temp_path("sema-test-rename");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));

    eval(&format!(r#"(file/write "{dir}/old.txt" "content")"#));
    eval(&format!(r#"(file/rename "{dir}/old.txt" "{dir}/new.txt")"#));
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/old.txt")"#)),
        Value::bool(false)
    );
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/new.txt")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/read "{dir}/new.txt")"#)),
        Value::string("content")
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_list_standalone() {
    let dir = temp_path("sema-test-list");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));

    eval(&format!(r#"(file/write "{dir}/a.txt" "a")"#));
    eval(&format!(r#"(file/write "{dir}/b.txt" "b")"#));
    eval(&format!(r#"(file/mkdir "{dir}/sub")"#));

    let listing = eval(&format!(r#"(file/list "{dir}")"#));
    if let Some(items) = listing.as_list() {
        let names: Vec<String> = items.iter().map(|v| v.to_string()).collect();
        assert!(names.iter().any(|n| n.contains("a.txt")));
        assert!(names.iter().any(|n| n.contains("b.txt")));
        assert!(names.iter().any(|n| n.contains("sub")));
    } else {
        panic!("file/list should return a list");
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_mkdir_standalone() {
    let dir = temp_path("sema-test-mkdir");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);

    // Recursive mkdir
    eval(&format!(r#"(file/mkdir "{dir}/a/b/c")"#));
    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}/a/b/c")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}/a/b")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}/a")"#)),
        Value::bool(true)
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_is_directory_standalone() {
    let dir = temp_path("sema-test-isdir");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));
    eval(&format!(r#"(file/write "{dir}/f.txt" "file")"#));

    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}/f.txt")"#)),
        Value::bool(false)
    );
    assert_eq!(
        eval(&format!(r#"(file/is-directory? "{dir}/nonexistent")"#)),
        Value::bool(false)
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_info_standalone() {
    let dir = temp_path("sema-test-info");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));
    eval(&format!(r#"(file/write "{dir}/test.txt" "hello")"#));

    // File info
    let info = eval(&format!(
        r#"(begin
            (define info (file/info "{dir}/test.txt"))
            (list (get info :size) (get info :is-file) (get info :is-dir)))"#
    ));
    if let Some(items) = info.as_list() {
        assert_eq!(items[0], Value::int(5)); // "hello" is 5 bytes
        assert_eq!(items[1], Value::bool(true));
        assert_eq!(items[2], Value::bool(false));
    } else {
        panic!("expected list from file/info test");
    }

    // Directory info
    let dir_info = eval(&format!(r#"(get (file/info "{dir}") :is-dir)"#));
    assert_eq!(dir_info, Value::bool(true));

    // :modified should be an integer
    let modified = eval(&format!(r#"(get (file/info "{dir}/test.txt") :modified)"#));
    assert!(modified.is_int());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_read_lines() {
    let dir = temp_path("sema-test-readlines");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));

    eval(&format!(
        r#"(file/write "{dir}/lines.txt" "alpha\nbeta\ngamma")"#
    ));
    let result = eval(&format!(r#"(file/read-lines "{dir}/lines.txt")"#));
    if let Some(items) = result.as_list() {
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::string("alpha"));
        assert_eq!(items[1], Value::string("beta"));
        assert_eq!(items[2], Value::string("gamma"));
    } else {
        panic!("file/read-lines should return a list");
    }

    // Empty file → empty list (no lines)
    eval(&format!(r#"(file/write "{dir}/empty.txt" "")"#));
    let empty = eval(&format!(r#"(file/read-lines "{dir}/empty.txt")"#));
    if let Some(items) = empty.as_list() {
        assert_eq!(items.len(), 0);
    } else {
        panic!("expected list");
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_write_lines() {
    let dir = temp_path("sema-test-writelines");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));

    eval(&format!(
        r#"(file/write-lines "{dir}/out.txt" (list "line1" "line2" "line3"))"#
    ));
    assert_eq!(
        eval(&format!(r#"(file/read "{dir}/out.txt")"#)),
        Value::string("line1\nline2\nline3")
    );

    // Roundtrip: write-lines then read-lines
    eval(&format!(
        r#"(file/write-lines "{dir}/round.txt" (list "a" "b" "c"))"#
    ));
    let result = eval(&format!(r#"(file/read-lines "{dir}/round.txt")"#));
    if let Some(items) = result.as_list() {
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::string("a"));
        assert_eq!(items[1], Value::string("b"));
        assert_eq!(items[2], Value::string("c"));
    } else {
        panic!("expected list");
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_for_each_line() {
    let dir = unique_temp_dir("foreach-line");
    let dir = dir.display().to_string();
    let dir = dir.as_str();
    eval(&format!(
        r#"(file/write "{dir}/data.txt" "alpha\nbeta\ngamma")"#
    ));

    // Count lines using for-each-line with a mutable counter
    let result = eval(&format!(
        r#"(begin
             (define count 0)
             (file/for-each-line "{dir}/data.txt"
               (fn (line) (set! count (+ count 1))))
             count)"#
    ));
    assert_eq!(result, Value::int(3));

    // Collect lines into a list via set!
    let collected = eval(&format!(
        r#"(begin
             (define lines '())
             (file/for-each-line "{dir}/data.txt"
               (fn (line) (set! lines (append lines (list line)))))
             lines)"#
    ));
    if let Some(items) = collected.as_list() {
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::string("alpha"));
        assert_eq!(items[1], Value::string("beta"));
        assert_eq!(items[2], Value::string("gamma"));
    } else {
        panic!("expected list, got: {collected}");
    }

    // Empty file — should call function zero times
    eval(&format!(r#"(file/write "{dir}/empty.txt" "")"#));
    let empty_count = eval(&format!(
        r#"(begin
             (define n 0)
             (file/for-each-line "{dir}/empty.txt"
               (fn (line) (set! n (+ n 1))))
             n)"#
    ));
    // BufReader.lines() on "" yields zero lines
    assert_eq!(empty_count, Value::int(0));

    // Single line, no trailing newline
    eval(&format!(r#"(file/write "{dir}/one.txt" "hello")"#));
    let one = eval(&format!(
        r#"(begin
             (define result '())
             (file/for-each-line "{dir}/one.txt"
               (fn (line) (set! result (append result (list line)))))
             result)"#
    ));
    if let Some(items) = one.as_list() {
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], Value::string("hello"));
    } else {
        panic!("expected list");
    }

    // Arity errors
    assert_arity_error(&format!(r#"(file/for-each-line "{dir}/data.txt")"#));
    assert_arity_error(r#"(file/for-each-line)"#);

    // Non-existent file → IO error
    assert!(matches!(
        eval_err(&format!(
            r#"(file/for-each-line "{}" (fn (l) l))"#,
            temp_path("nonexistent-sema.txt")
        ))
        .inner(),
        SemaError::Io(_)
    ));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_fold_lines() {
    let dir = unique_temp_dir("fold-lines");
    let dir = dir.display().to_string();
    let dir = dir.as_str();
    eval(&format!(r#"(file/write "{dir}/nums.txt" "10\n20\n30")"#));

    // Sum numbers using fold-lines
    let result = eval(&format!(
        r#"(file/fold-lines "{dir}/nums.txt"
             (fn (acc line) (+ acc (string->number line)))
             0)"#
    ));
    assert_eq!(result, Value::int(60));

    // Count lines
    let count = eval(&format!(
        r#"(file/fold-lines "{dir}/nums.txt"
             (fn (n line) (+ n 1))
             0)"#
    ));
    assert_eq!(count, Value::int(3));

    // Collect lines into a list
    let collected = eval(&format!(
        r#"(file/fold-lines "{dir}/nums.txt"
             (fn (acc line) (append acc (list line)))
             '())"#
    ));
    if let Some(items) = collected.as_list() {
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::string("10"));
        assert_eq!(items[1], Value::string("20"));
        assert_eq!(items[2], Value::string("30"));
    } else {
        panic!("expected list, got: {collected}");
    }

    // Empty file — returns initial accumulator
    eval(&format!(r#"(file/write "{dir}/empty.txt" "")"#));
    let empty = eval(&format!(
        r#"(file/fold-lines "{dir}/empty.txt"
             (fn (acc line) (+ acc 1))
             42)"#
    ));
    assert_eq!(empty, Value::int(42));

    // Build a map from key=value lines
    eval(&format!(
        r#"(file/write "{dir}/kv.txt" "name=alice\nage=30\ncity=paris")"#
    ));
    let map_result = eval(&format!(
        r#"(file/fold-lines "{dir}/kv.txt"
             (fn (acc line)
               (let ((parts (string/split line "=")))
                 (assoc acc (first parts) (nth parts 1))))
             {{}})"#
    ));
    if let Some(m) = map_result.as_map_rc() {
        assert_eq!(m.get(&Value::string("name")), Some(&Value::string("alice")));
        assert_eq!(m.get(&Value::string("age")), Some(&Value::string("30")));
        assert_eq!(m.get(&Value::string("city")), Some(&Value::string("paris")));
    } else {
        panic!("expected map, got: {map_result}");
    }

    // Arity errors
    assert_arity_error(r#"(file/fold-lines "f" (fn (a b) a))"#);
    assert_arity_error(r#"(file/fold-lines)"#);

    // Non-existent file → IO error
    assert!(matches!(
        eval_err(&format!(
            r#"(file/fold-lines "{}" (fn (a l) a) 0)"#,
            temp_path("nonexistent-sema.txt")
        ))
        .inner(),
        SemaError::Io(_)
    ));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_fold_lines_bytes() {
    let dir = unique_temp_dir("fold-lines-bytes");
    let dir = dir.display().to_string();
    let dir = dir.as_str();
    // Mixed \n / \r\n endings and one-decimal / no-decimal temperatures —
    // the exact shape of a 1BRC measurements file.
    std::fs::write(
        format!("{dir}/temps.txt"),
        "Oslo;-12.3\r\nBergen;5\nTromso;0.0\n",
    )
    .expect("write fixture");

    // Sum temperatures as scaled ints via bytes/parse-int10 (int*10 trick):
    // -123 + 50 + 0 = -73. Lines arrive as bytevectors with \n and \r stripped.
    let sum = eval(&format!(
        r#"(file/fold-lines-bytes "{dir}/temps.txt"
             (fn (acc line)
               (let ((semi (bytes/find line 59)))
                 (+ acc (bytes/parse-int10 line (+ semi 1)))))
             0)"#
    ));
    assert_eq!(sum, Value::int(-73));

    // Station names decode from the byte prefix; accumulate into a
    // mutable array and freeze at the end.
    let names = eval(&format!(
        r#"(mutable-array/->vector
             (file/fold-lines-bytes "{dir}/temps.txt"
               (fn (acc line)
                 (mutable-array/push! acc (bytes/->string line 0 (bytes/find line 59))))
               (mutable-array/new)))"#
    ));
    assert_eq!(names, eval(r#"["Oslo" "Bergen" "Tromso"]"#));

    // Empty file — returns the initial accumulator untouched.
    std::fs::write(format!("{dir}/empty.txt"), "").expect("write fixture");
    let empty = eval(&format!(
        r#"(file/fold-lines-bytes "{dir}/empty.txt" (fn (acc line) (+ acc 1)) 42)"#
    ));
    assert_eq!(empty, Value::int(42));

    // CR parity with file/fold-lines: \r is only stripped as part of a \r\n
    // pair, so a final unterminated line ending in a bare \r keeps it as
    // content. Line lengths must match the string sibling's exactly.
    std::fs::write(format!("{dir}/cr.txt"), "line1\r\nline2\rmid\nlast\r").expect("write fixture");
    let byte_lens = eval(&format!(
        r#"(file/fold-lines-bytes "{dir}/cr.txt"
             (fn (acc line) (cons (bytes/length line) acc))
             '())"#
    ));
    let str_lens = eval(&format!(
        r#"(file/fold-lines "{dir}/cr.txt"
             (fn (acc line) (cons (string-length line) acc))
             '())"#
    ));
    assert_eq!(byte_lens, eval("'(5 9 5)"));
    assert_eq!(byte_lens, str_lens);

    // Arity errors
    assert_arity_error(r#"(file/fold-lines-bytes "f" (fn (a b) a))"#);
    assert_arity_error(r#"(file/fold-lines-bytes)"#);

    // Non-existent file → IO error
    assert!(matches!(
        eval_err(&format!(
            r#"(file/fold-lines-bytes "{}" (fn (a l) a) 0)"#,
            temp_path("nonexistent-sema-bytes.txt")
        ))
        .inner(),
        SemaError::Io(_)
    ));

    let _ = std::fs::remove_dir_all(dir);
}

/// Pins the exact aggregate line of `benchmarks/1brc/1brc.sema` on an
/// adversarial fixture: negative temps, -0.x forms, a single-row station,
/// a multi-byte UTF-8 name, a mean landing on a round-half tie
/// (ties-to-even), and a blank line (skipped). The expected string is what
/// a float/string reference implementation (string/split + string->float +
/// float stats) produces on the same fixture — the correctness oracle for
/// the benchmark's byte-oriented int*10 hot loop.
#[test]
fn test_1brc_benchmark_output() {
    let dir = unique_temp_dir("1brc-output");
    let fixture = dir.join("measurements.txt");
    std::fs::write(
        &fixture,
        "Oslo;-12.3\nOslo;-0.3\nOslo;0.0\nKuala Lumpur;27.8\nKuala Lumpur;27.9\n\
         São Paulo;20.1\nSolo;-7.8\nLima;5.0\nTie;1.0\nTie;1.1\n\
         T2;0.3\nT2;0.3\nT2;0.3\nHot;99.9\nCold;-99.9\nCold;-99.8\n\
         NegMean;-0.1\nNegMean;-0.2\n\nZed;0.1\n",
    )
    .expect("write fixture");

    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../benchmarks/1brc/1brc.sema"
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([script, "--", fixture.to_str().unwrap()])
        .output()
        .expect("failed to run 1brc.sema");
    assert!(
        output.status.success(),
        "1brc.sema failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let aggregate = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .expect("no aggregate line in 1brc output");
    assert_eq!(
        aggregate,
        "{Cold=-99.9/-99.8/-99.8, Hot=99.9/99.9/99.9, Kuala Lumpur=27.8/27.8/27.9, \
         Lima=5.0/5.0/5.0, NegMean=-0.2/-0.2/-0.1, Oslo=-12.3/-4.2/0.0, \
         Solo=-7.8/-7.8/-7.8, São Paulo=20.1/20.1/20.1, T2=0.3/0.3/0.3, \
         Tie=1.0/1.0/1.1, Zed=0.1/0.1/0.1}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_file_copy() {
    let dir = temp_path("sema-test-copy");
    let dir = dir.as_str();
    let _ = std::fs::remove_dir_all(dir);
    eval(&format!(r#"(file/mkdir "{dir}")"#));

    eval(&format!(r#"(file/write "{dir}/src.txt" "original")"#));
    eval(&format!(r#"(file/copy "{dir}/src.txt" "{dir}/dest.txt")"#));

    // Both files exist with same content
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/src.txt")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/exists? "{dir}/dest.txt")"#)),
        Value::bool(true)
    );
    assert_eq!(
        eval(&format!(r#"(file/read "{dir}/dest.txt")"#)),
        Value::string("original")
    );

    // Source is unchanged
    assert_eq!(
        eval(&format!(r#"(file/read "{dir}/src.txt")"#)),
        Value::string("original")
    );

    // Copy non-existent file should error
    let err = eval_err(&format!(
        r#"(file/copy "{dir}/nope.txt" "{dir}/dest2.txt")"#
    ));
    assert!(err.to_string().contains("file/copy"));

    let _ = std::fs::remove_dir_all(dir);
}

// Path operations (extended): join, dirname, basename, extension, absolute

#[test]
fn test_path_join_extended() {
    assert_eq!(eval_path(r#"(path/join "a" "b" "c")"#), "a/b/c");
    assert_eq!(
        eval_path(r#"(path/join "/usr" "local" "bin")"#),
        "/usr/local/bin"
    );
    // Single arg
    assert_eq!(eval_path(r#"(path/join "only")"#), "only");
}

// Regression: test helpers must stay platform-agnostic. `temp_path` yields a
// forward-slash path under the OS temp dir (no hardcoded `/tmp`), and `eval_path`
// normalizes the `\` that `path/join` emits on Windows. See OPEN.md
// platform-specific-windows.
#[test]
fn test_temp_path_helper_is_forward_slash_under_temp_dir() {
    let p = temp_path("sema-helper-regression.txt");
    assert!(!p.contains('\\'), "temp_path must use forward slashes: {p}");
    let expected_root = std::env::temp_dir().to_string_lossy().replace('\\', "/");
    assert!(
        p.starts_with(&expected_root),
        "temp_path must live under temp_dir: {p} (root {expected_root})"
    );
    assert!(p.ends_with("/sema-helper-regression.txt"));
}

#[test]
fn test_eval_path_helper_normalizes_separators() {
    // Regardless of the host separator, the joined components compare equal.
    assert_eq!(eval_path(r#"(path/join "x" "y" "z")"#), "x/y/z");
}

#[test]
fn test_path_dirname_extended() {
    assert_eq!(
        eval(r#"(path/dirname "/a/b/c.txt")"#),
        Value::string("/a/b")
    );
    assert_eq!(eval(r#"(path/dirname "file.txt")"#), Value::string(""));
    assert_eq!(eval(r#"(path/dirname "/root")"#), Value::string("/"));
}

#[test]
fn test_path_basename_extended() {
    assert_eq!(
        eval(r#"(path/basename "/a/b/c.txt")"#),
        Value::string("c.txt")
    );
    assert_eq!(
        eval(r#"(path/basename "plain.rs")"#),
        Value::string("plain.rs")
    );
    assert_eq!(eval(r#"(path/basename "/a/b/")"#), Value::string("b"));
}

#[test]
fn test_path_extension_extended() {
    assert_eq!(
        eval(r#"(path/extension "file.tar.gz")"#),
        Value::string("gz")
    );
    assert_eq!(eval(r#"(path/extension "Makefile")"#), Value::string(""));
    // ".hidden" has no extension in Rust's Path semantics
    assert_eq!(eval(r#"(path/extension ".hidden")"#), Value::string(""));
}

#[test]
fn test_path_absolute_extended() {
    // path/absolute on an existing path returns a string
    let result = eval(r#"(path/absolute ".")"#);
    if let Some(s) = result.as_str() {
        #[cfg(unix)]
        assert!(s.starts_with('/'));
        #[cfg(windows)]
        assert!(s.contains(":\\") || s.contains(":/"));
    } else {
        panic!("path/absolute should return a string");
    }
    // Non-existent path should error
    let err = eval_err(r#"(path/absolute "/nonexistent_sema_test_path_xyz")"#);
    assert!(err.to_string().contains("path/absolute"));
}

// String: trim-left, trim-right, number?

#[test]
fn test_string_trim_left() {
    assert_eq!(
        eval(r#"(string/trim-left "  hello  ")"#),
        Value::string("hello  ")
    );
    assert_eq!(eval(r#"(string/trim-left "\t\n hi")"#), Value::string("hi"));
    assert_eq!(
        eval(r#"(string/trim-left "no-space")"#),
        Value::string("no-space")
    );
}

#[test]
fn test_string_trim_right() {
    assert_eq!(
        eval(r#"(string/trim-right "  hello  ")"#),
        Value::string("  hello")
    );
    assert_eq!(
        eval(r#"(string/trim-right "hi\t\n ")"#),
        Value::string("hi")
    );
    assert_eq!(
        eval(r#"(string/trim-right "no-space")"#),
        Value::string("no-space")
    );
}

#[test]
fn test_string_number_predicate() {
    assert_eq!(eval(r#"(string/number? "42")"#), Value::bool(true));
    assert_eq!(eval(r#"(string/number? "-7")"#), Value::bool(true));
    assert_eq!(eval(r#"(string/number? "3.14")"#), Value::bool(true));
    assert_eq!(eval(r#"(string/number? "-0.5")"#), Value::bool(true));
    assert_eq!(eval(r#"(string/number? "hello")"#), Value::bool(false));
    assert_eq!(eval(r#"(string/number? "")"#), Value::bool(false));
    assert_eq!(eval(r#"(string/number? "12abc")"#), Value::bool(false));
}

// Map: map-keys, from-entries

#[test]
fn test_map_map_keys() {
    // Assert via map/entries (deterministic BTreeMap iteration) rather than the
    // raw `{...}` display string, so the test is resilient to map display tweaks.
    assert_eq!(
        eval_to_string(r#"(map/entries (map/map-keys keyword->string {:a 1 :b 2}))"#),
        r#"(("a" 1) ("b" 2))"#
    );
}

#[test]
fn test_map_from_entries() {
    // Assert via map/entries (deterministic BTreeMap iteration) rather than the
    // raw `{...}` display string, so the test is resilient to map display tweaks.
    assert_eq!(
        eval_to_string(r#"(map/entries (map/from-entries (list (list :a 1) (list :b 2))))"#),
        "((:a 1) (:b 2))"
    );
    // Empty list -> empty map
    assert_eq!(
        eval_to_string(r#"(map/entries (map/from-entries (list)))"#),
        "()"
    );
    // Roundtrip: entries -> from-entries
    assert_eq!(
        eval_to_string(r#"(map/entries (map/from-entries (map/entries {:x 10 :y 20})))"#),
        "((:x 10) (:y 20))"
    );
}

#[test]
fn test_map_from_entries_error() {
    // Entry with wrong size should error
    let err = eval_err(r#"(map/from-entries (list (list :a 1 :extra)))"#);
    assert!(err.to_string().contains("pair"));
}

// Extended tests for existing list functions: any, every, reduce,
// partition, foldr, member

#[test]
fn test_any_with_even_predicate() {
    assert_eq!(eval("(any even? (list 1 3 4))"), Value::bool(true));
    assert_eq!(eval("(any even? (list 1 3 5))"), Value::bool(false));
    // empty list → false
    assert_eq!(eval("(any even? (list))"), Value::bool(false));
    // works on vectors
    assert_eq!(eval("(any even? [1 2 3])"), Value::bool(true));
    // short-circuits: only first truthy hit matters
    assert_eq!(
        eval("(any (lambda (x) (> x 0)) '(1 2 3))"),
        Value::bool(true)
    );
}

#[test]
fn test_every_with_even_predicate() {
    assert_eq!(eval("(every even? (list 2 4 6))"), Value::bool(true));
    assert_eq!(eval("(every even? (list 2 3 6))"), Value::bool(false));
    // empty list → true (vacuous truth)
    assert_eq!(eval("(every even? (list))"), Value::bool(true));
    // works on vectors
    assert_eq!(eval("(every even? [2 4 6])"), Value::bool(true));
}

#[test]
fn test_reduce_edge_cases() {
    // single element list → returns that element
    assert_eq!(eval("(reduce + '(42))"), Value::int(42));
    // string concatenation
    assert_eq!(
        eval(r#"(reduce string-append '("a" "b" "c"))"#),
        Value::string("abc")
    );
    // empty list → error
    let err = eval_err("(reduce + '())");
    assert!(err.to_string().contains("empty"));
}

#[test]
fn test_partition_extended() {
    // all match
    assert_eq!(eval_to_string("(partition even? '(2 4 6))"), "((2 4 6) ())");
    // none match
    assert_eq!(eval_to_string("(partition even? '(1 3 5))"), "(() (1 3 5))");
    // empty list
    assert_eq!(eval_to_string("(partition even? '())"), "(() ())");
    // works on vectors
    assert_eq!(
        eval_to_string("(partition even? [1 2 3 4])"),
        "((2 4) (1 3))"
    );
}

#[test]
fn test_foldr_extended() {
    // right fold builds list in original order with cons
    assert_eq!(eval_to_string("(foldr cons '() '(1 2 3))"), "(1 2 3)");
    // subtraction shows right-associativity: 1 - (2 - (3 - 0)) = 1 - (2 - 3) = 1 - (-1) = 2
    assert_eq!(eval("(foldr - 0 '(1 2 3))"), Value::int(2));
    // empty list → returns init
    assert_eq!(eval("(foldr + 99 '())"), Value::int(99));
}

#[test]
fn test_member_extended() {
    // found at beginning
    assert_eq!(eval_to_string("(member 1 '(1 2 3))"), "(1 2 3)");
    // found at end
    assert_eq!(eval_to_string("(member 3 '(1 2 3))"), "(3)");
    // not found → #f
    assert_eq!(eval("(member 99 '(1 2 3))"), Value::bool(false));
    // empty list → #f
    assert_eq!(eval("(member 1 '())"), Value::bool(false));
    // works with keywords
    assert_eq!(eval_to_string("(member :b '(:a :b :c))"), "(:b :c)");
}

// New list functions: sort-by, flatten-deep, interpose, frequencies,
// list->vector, vector->list

#[test]
fn test_sort_by() {
    // sort by absolute value
    assert_eq!(
        eval_to_string("(sort-by (lambda (x) (if (< x 0) (- 0 x) x)) '(3 -1 2 -5 4))"),
        "(-1 2 3 4 -5)"
    );
    // sort strings by length
    assert_eq!(
        eval_to_string(r#"(sort-by string-length '("bb" "a" "ccc"))"#),
        r#"("a" "bb" "ccc")"#
    );
    // empty list
    assert_eq!(eval_to_string("(sort-by (lambda (x) x) '())"), "()");
    // single element
    assert_eq!(eval_to_string("(sort-by (lambda (x) x) '(42))"), "(42)");
    // works on vectors
    assert_eq!(
        eval_to_string("(sort-by (lambda (x) x) [3 1 2])"),
        "(1 2 3)"
    );
}

#[test]
fn test_flatten_deep_fn() {
    // deeply nested lists
    assert_eq!(
        eval_to_string("(flatten-deep '(1 (2 (3 (4 5))) 6))"),
        "(1 2 3 4 5 6)"
    );
    // already flat
    assert_eq!(eval_to_string("(flatten-deep '(1 2 3))"), "(1 2 3)");
    // empty
    assert_eq!(eval_to_string("(flatten-deep '())"), "()");
    // mixed lists and vectors
    assert_eq!(
        eval_to_string("(flatten-deep '(1 [2 [3]] (4)))"),
        "(1 2 3 4)"
    );
    // single nested element
    assert_eq!(eval_to_string("(flatten-deep '(((42))))"), "(42)");
}

#[test]
fn test_interpose() {
    // basic usage
    assert_eq!(eval_to_string("(interpose :x '(1 2 3))"), "(1 :x 2 :x 3)");
    // single element → no separator
    assert_eq!(eval_to_string("(interpose :x '(1))"), "(1)");
    // empty list
    assert_eq!(eval_to_string("(interpose :x '())"), "()");
    // string separator
    assert_eq!(
        eval_to_string(r#"(interpose ", " '(1 2 3))"#),
        r#"(1 ", " 2 ", " 3)"#
    );
    // works on vectors
    assert_eq!(eval_to_string("(interpose 0 [1 2 3])"), "(1 0 2 0 3)");
}

#[test]
fn test_frequencies() {
    // Assert counts via map/entries (deterministic BTreeMap iteration) rather
    // than the raw `{...}` display string, so the test is resilient to map
    // display tweaks while still verifying both keys and counts.
    assert_eq!(
        eval_to_string("(map/entries (frequencies '(:a :b :a)))"),
        "((:a 2) (:b 1))"
    );
    // all unique
    assert_eq!(
        eval_to_string("(map/entries (frequencies '(1 2 3)))"),
        "((1 1) (2 1) (3 1))"
    );
    // all same
    assert_eq!(
        eval_to_string("(map/entries (frequencies '(1 1 1)))"),
        "((1 3))"
    );
    // empty list
    assert_eq!(eval_to_string("(map/entries (frequencies '()))"), "()");
    // works on vectors
    assert_eq!(
        eval_to_string("(map/entries (frequencies [1 2 1]))"),
        "((1 2) (2 1))"
    );
}

#[test]
fn test_list_to_vector() {
    // basic conversion
    assert_eq!(eval_to_string("(list->vector '(1 2 3))"), "[1 2 3]");
    // empty list
    assert_eq!(eval_to_string("(list->vector '())"), "[]");
    // type error on non-list
    let err = eval_err("(list->vector [1 2 3])");
    assert!(err.to_string().contains("list"));
}

#[test]
fn test_vector_to_list() {
    // basic conversion
    assert_eq!(eval_to_string("(vector->list [1 2 3])"), "(1 2 3)");
    // empty vector
    assert_eq!(eval_to_string("(vector->list [])"), "()");
    // type error on non-vector
    let err = eval_err("(vector->list '(1 2 3))");
    assert!(err.to_string().contains("vector"));
}

#[test]
fn test_sort_by_arity_error() {
    let err = eval_err("(sort-by '(1 2))");
    assert!(err.to_string().contains("2"));
}

#[test]
fn test_interpose_arity_error() {
    let err = eval_err("(interpose :x)");
    assert!(err.to_string().contains("2"));
}

#[test]
fn test_frequencies_arity_error() {
    let err = eval_err("(frequencies '(1) '(2))");
    assert!(err.to_string().contains("1"));
}

// Crypto: hash/md5, hash/hmac-sha256

#[test]
fn test_hash_md5() {
    // MD5 of empty string
    assert_eq!(
        eval(r#"(hash/md5 "")"#),
        Value::string("d41d8cd98f00b204e9800998ecf8427e")
    );
    // MD5 of "hello"
    assert_eq!(
        eval(r#"(hash/md5 "hello")"#),
        Value::string("5d41402abc4b2a76b9719d911017c592")
    );
}

#[test]
fn test_hash_md5_errors() {
    assert_arity_error(r#"(hash/md5)"#);
    assert_type_error(r#"(hash/md5 1)"#);
}

#[test]
fn test_hash_hmac_sha256() {
    // Known HMAC-SHA256 test vector
    let result = eval(r#"(hash/hmac-sha256 "key" "message")"#);
    assert!(result.is_string());
    if let Some(s) = result.as_str() {
        assert_eq!(s.len(), 64); // 32 bytes = 64 hex chars
    }
}

#[test]
fn test_hash_hmac_sha256_errors() {
    assert_arity_error(r#"(hash/hmac-sha256 "key")"#);
    assert_type_error(r#"(hash/hmac-sha256 1 "msg")"#);
}

// Datetime: time/add, time/diff

#[test]
fn test_time_add() {
    assert_eq!(eval("(time/add 1000.0 500.0)"), Value::float(1500.0));
    assert_eq!(eval("(time/add 1000 60)"), Value::float(1060.0));
}

#[test]
fn test_time_add_errors() {
    assert_arity_error(r#"(time/add 1000)"#);
    assert_type_error(r#"(time/add "x" 1)"#);
}

#[test]
fn test_time_diff() {
    assert_eq!(eval("(time/diff 1500.0 1000.0)"), Value::float(500.0));
    assert_eq!(eval("(time/diff 1000 1500)"), Value::float(-500.0));
}

#[test]
fn test_time_diff_errors() {
    assert_arity_error(r#"(time/diff 1000)"#);
}

// System: sys/set-env

#[test]
fn test_sys_set_env() {
    let var_name = format!("SEMA_TEST_VAR_{}", std::process::id());
    assert_eq!(
        eval(&format!(r#"(sys/set-env "{var_name}" "hello")"#)),
        Value::nil()
    );
    assert_eq!(
        eval(&format!(r#"(env "{var_name}")"#)),
        Value::string("hello")
    );
    // Clean up
    std::env::remove_var(&var_name);
}

#[test]
fn test_sys_set_env_errors() {
    assert_arity_error(r#"(sys/set-env "X")"#);
    assert_type_error(r#"(sys/set-env 1 "val")"#);
}

// LLM Data Types: Prompts, Messages, Conversations, Tools, Agents

#[test]
fn test_prompt_creation() {
    // Prompt is created via special form
    let result = eval_to_string(r#"(prompt (user "hello"))"#);
    assert!(result.contains("prompt"));
}

#[test]
fn test_prompt_predicate() {
    assert_eq!(
        eval(r#"(prompt? (prompt (user "hello")))"#),
        Value::bool(true)
    );
    assert_eq!(eval(r#"(prompt? 42)"#), Value::bool(false));
    assert_eq!(eval(r#"(prompt? "not a prompt")"#), Value::bool(false));
    assert_eq!(eval(r#"(prompt? nil)"#), Value::bool(false));
}

#[test]
fn test_prompt_messages() {
    assert_eq!(
        eval(r#"(length (prompt/messages (prompt (user "hello") (assistant "hi"))))"#),
        Value::int(2)
    );
}

#[test]
fn test_prompt_append() {
    assert_eq!(
        eval(
            r#"(length (prompt/messages (prompt/append (prompt (user "a")) (prompt (assistant "b")))))"#
        ),
        Value::int(2)
    );
}

#[test]
fn test_prompt_set_system() {
    // prompt/set-system replaces system message
    let result = eval(
        r#"
        (begin
          (define p (prompt (system "old") (user "hello")))
          (define p2 (prompt/set-system p "new system"))
          (length (prompt/messages p2)))
    "#,
    );
    assert_eq!(result, Value::int(2));
}

#[test]
fn test_prompt_set_system_adds_when_missing() {
    // If no system message, it adds one at front
    let result = eval(
        r#"
        (begin
          (define p (prompt (user "hello")))
          (define p2 (prompt/set-system p "system msg"))
          (length (prompt/messages p2)))
    "#,
    );
    assert_eq!(result, Value::int(2));
}

#[test]
fn test_message_creation() {
    let result = eval_to_string(r#"(message :user "hello world")"#);
    assert!(result.contains("message"));
}

#[test]
fn test_message_predicate() {
    assert_eq!(
        eval(r#"(message? (message :user "hi"))"#),
        Value::bool(true)
    );
    assert_eq!(eval(r#"(message? 42)"#), Value::bool(false));
    assert_eq!(eval(r#"(message? "not a message")"#), Value::bool(false));
}

#[test]
fn test_message_role() {
    assert_eq!(
        eval(r#"(message/role (message :user "hi"))"#),
        Value::keyword("user")
    );
    assert_eq!(
        eval(r#"(message/role (message :assistant "hello"))"#),
        Value::keyword("assistant")
    );
    assert_eq!(
        eval(r#"(message/role (message :system "you are helpful"))"#),
        Value::keyword("system")
    );
}

#[test]
fn test_message_content() {
    assert_eq!(
        eval(r#"(message/content (message :user "hello world"))"#),
        Value::string("hello world")
    );
    assert_eq!(
        eval(r#"(message/content (message :assistant "response"))"#),
        Value::string("response")
    );
}

#[test]
fn test_message_from_prompt() {
    // Extract messages from prompt, check role and content
    assert_eq!(
        eval(
            r#"
            (begin
              (define p (prompt (user "test input")))
              (define msgs (prompt/messages p))
              (message/content (car msgs)))
        "#
        ),
        Value::string("test input")
    );
    assert_eq!(
        eval(
            r#"
            (begin
              (define p (prompt (user "test input")))
              (define msgs (prompt/messages p))
              (message/role (car msgs)))
        "#
        ),
        Value::keyword("user")
    );
}

#[test]
fn test_conversation_new() {
    let result = eval_to_string(r#"(conversation/new {:model "test-model"})"#);
    assert!(result.contains("conversation"));
}

#[test]
fn test_conversation_new_empty() {
    let result = eval_to_string(r#"(conversation/new)"#);
    assert!(result.contains("conversation"));
}

#[test]
fn test_conversation_predicate() {
    assert_eq!(
        eval(r#"(conversation? (conversation/new))"#),
        Value::bool(true)
    );
    assert_eq!(eval(r#"(conversation? 42)"#), Value::bool(false));
    assert_eq!(eval(r#"(conversation? "not a conv")"#), Value::bool(false));
}

#[test]
fn test_conversation_messages_empty() {
    assert_eq!(
        eval(r#"(length (conversation/messages (conversation/new)))"#),
        Value::int(0)
    );
}

#[test]
fn test_conversation_fork() {
    // fork returns a copy
    assert_eq!(
        eval(r#"(conversation? (conversation/fork (conversation/new)))"#),
        Value::bool(true)
    );
}

#[test]
fn test_conversation_model() {
    assert_eq!(
        eval(r#"(conversation/model (conversation/new {:model "gpt-4"}))"#),
        Value::string("gpt-4")
    );
}

#[test]
fn test_conversation_model_empty() {
    assert_eq!(
        eval(r#"(conversation/model (conversation/new))"#),
        Value::string("")
    );
}

#[test]
fn test_conversation_add_message() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define c (conversation/new))
              (define c2 (conversation/add-message c :user "hello"))
              (length (conversation/messages c2)))
        "#
        ),
        Value::int(1)
    );
}

#[test]
fn test_conversation_add_message_multiple() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define c (conversation/new {:model "test"}))
              (define c1 (conversation/add-message c :user "hello"))
              (define c2 (conversation/add-message c1 :assistant "hi there"))
              (define c3 (conversation/add-message c2 :user "how are you?"))
              (length (conversation/messages c3)))
        "#
        ),
        Value::int(3)
    );
}

#[test]
fn test_conversation_add_message_preserves_model() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define c (conversation/new {:model "gpt-4"}))
              (define c2 (conversation/add-message c :user "hello"))
              (conversation/model c2))
        "#
        ),
        Value::string("gpt-4")
    );
}

#[test]
fn test_conversation_add_message_immutable() {
    // Original conversation should not be modified
    assert_eq!(
        eval(
            r#"
            (begin
              (define c (conversation/new))
              (define c2 (conversation/add-message c :user "hello"))
              (length (conversation/messages c)))
        "#
        ),
        Value::int(0)
    );
}

#[test]
fn test_conversation_add_message_content_check() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define c (conversation/add-message (conversation/new) :user "hello"))
              (define msgs (conversation/messages c))
              (message/content (car msgs)))
        "#
        ),
        Value::string("hello")
    );
}

#[test]
fn test_conversation_add_message_role_check() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define c (conversation/add-message (conversation/new) :assistant "response"))
              (define msgs (conversation/messages c))
              (message/role (car msgs)))
        "#
        ),
        Value::keyword("assistant")
    );
}

#[test]
fn test_tool_predicate() {
    assert_eq!(
        eval(
            r#"(begin (deftool my-test-tool-1 "a tool" {:x {:type :string}} (lambda (args) "ok")) (tool? my-test-tool-1))"#
        ),
        Value::bool(true)
    );
    assert_eq!(eval(r#"(tool? 42)"#), Value::bool(false));
    assert_eq!(eval(r#"(tool? "not a tool")"#), Value::bool(false));
}

#[test]
fn test_tool_name() {
    assert_eq!(
        eval(
            r#"(begin (deftool my-test-tool-2 "desc" {:x {:type :string}} (lambda (args) "ok")) (tool/name my-test-tool-2))"#
        ),
        Value::string("my-test-tool-2")
    );
}

#[test]
fn test_tool_description() {
    assert_eq!(
        eval(
            r#"(begin (deftool my-test-tool-3 "my description" {:x {:type :string}} (lambda (args) "ok")) (tool/description my-test-tool-3))"#
        ),
        Value::string("my description")
    );
}

#[test]
fn test_tool_parameters() {
    assert_eq!(
        eval(
            r#"(begin (deftool my-test-tool-4 "desc" {:x {:type :string}} (lambda (args) "ok")) (map? (tool/parameters my-test-tool-4)))"#
        ),
        Value::bool(true)
    );
}

#[test]
fn test_agent_predicate() {
    assert_eq!(
        eval(
            r#"(begin (defagent test-agent-1 {:system "helpful" :tools [] :model "gpt-4"}) (agent? test-agent-1))"#
        ),
        Value::bool(true)
    );
    assert_eq!(eval(r#"(agent? 42)"#), Value::bool(false));
    assert_eq!(eval(r#"(agent? "not an agent")"#), Value::bool(false));
}

#[test]
fn test_agent_name() {
    assert_eq!(
        eval(
            r#"(begin (defagent test-agent-2 {:system "sys" :tools []}) (agent/name test-agent-2))"#
        ),
        Value::string("test-agent-2")
    );
}

#[test]
fn test_agent_system() {
    assert_eq!(
        eval(
            r#"(begin (defagent test-agent-3 {:system "you are helpful" :tools []}) (agent/system test-agent-3))"#
        ),
        Value::string("you are helpful")
    );
}

#[test]
fn test_agent_tools_empty() {
    assert_eq!(
        eval(
            r#"(begin (defagent test-agent-4 {:system "sys" :tools []}) (length (agent/tools test-agent-4)))"#
        ),
        Value::int(0)
    );
}

#[test]
fn test_agent_model() {
    assert_eq!(
        eval(
            r#"(begin (defagent test-agent-5 {:system "sys" :tools [] :model "claude-3"}) (agent/model test-agent-5))"#
        ),
        Value::string("claude-3")
    );
}

#[test]
fn test_agent_max_turns_default() {
    assert_eq!(
        eval(
            r#"(begin (defagent test-agent-6 {:system "sys" :tools []}) (agent/max-turns test-agent-6))"#
        ),
        Value::int(10)
    );
}

#[test]
fn test_agent_max_turns_custom() {
    assert_eq!(
        eval(
            r#"(begin (defagent test-agent-7 {:system "sys" :tools [] :max-turns 5}) (agent/max-turns test-agent-7))"#
        ),
        Value::int(5)
    );
}

#[test]
fn test_agent_with_tools() {
    assert_eq!(
        eval(
            r#"
            (begin
              (deftool agent-tool-1 "tool1" {:x {:type :string}} (lambda (args) "ok"))
              (defagent test-agent-8 {:system "sys" :tools [agent-tool-1]})
              (length (agent/tools test-agent-8)))
        "#
        ),
        Value::int(1)
    );
}

#[test]
fn test_llm_similarity_identical() {
    assert_eq!(
        eval("(llm/similarity (list 1.0 0.0 0.0) (list 1.0 0.0 0.0))"),
        Value::float(1.0)
    );
}

#[test]
fn test_llm_similarity_orthogonal() {
    assert_eq!(
        eval("(llm/similarity (list 1.0 0.0) (list 0.0 1.0))"),
        Value::float(0.0)
    );
}

#[test]
fn test_llm_similarity_opposite() {
    assert_eq!(
        eval("(llm/similarity (list 1.0 0.0) (list -1.0 0.0))"),
        Value::float(-1.0)
    );
}

#[test]
fn test_llm_similarity_error_different_lengths() {
    let err = eval_err("(llm/similarity (list 1.0 0.0) (list 1.0 0.0 0.0))");
    assert!(err.to_string().contains("same length"));
}

#[test]
fn test_llm_similarity_error_empty() {
    let err = eval_err("(llm/similarity (list) (list))");
    assert!(err.to_string().contains("empty"));
}

#[test]
fn test_embedding_list_roundtrip() {
    // Convert list -> embedding bytevector -> list, verify roundtrip
    assert_eq!(
        eval_to_string("(embedding/->list (embedding/list->embedding '(1.0 2.0 3.0)))"),
        "(1.0 2.0 3.0)"
    );
}

#[test]
fn test_embedding_length() {
    assert_eq!(
        eval("(embedding/length (embedding/list->embedding '(1.0 2.0 3.0)))"),
        Value::int(3)
    );
}

#[test]
fn test_embedding_ref() {
    assert_eq!(
        eval("(embedding/ref (embedding/list->embedding '(10.5 20.5 30.5)) 1)"),
        Value::float(20.5)
    );
}

#[test]
fn test_embedding_ref_out_of_bounds() {
    let err = eval_err("(embedding/ref (embedding/list->embedding '(1.0 2.0)) 5)");
    assert!(err.to_string().contains("out of bounds"));
}

#[test]
fn test_embedding_similarity_bytevectors() {
    // Bytevector-based similarity (same as list-based, but through bytevector path)
    assert_eq!(
        eval("(llm/similarity (embedding/list->embedding '(1.0 0.0 0.0)) (embedding/list->embedding '(1.0 0.0 0.0)))"),
        Value::float(1.0)
    );
    assert_eq!(
        eval("(llm/similarity (embedding/list->embedding '(1.0 0.0)) (embedding/list->embedding '(0.0 1.0)))"),
        Value::float(0.0)
    );
}

#[test]
fn test_embedding_similarity_mixed_error() {
    let err = eval_err("(llm/similarity (embedding/list->embedding '(1.0 0.0)) '(0.0 1.0))");
    assert!(err.to_string().contains("same type"));
}

#[test]
fn test_embedding_list_to_embedding_preserves_values() {
    // Verify f64 encoding precision
    assert_eq!(
        eval("(embedding/ref (embedding/list->embedding '(3.14159265358979)) 0)"),
        Value::float(3.14159265358979)
    );
}

#[test]
fn test_embedding_integers_coerced() {
    // Integers should be accepted and coerced to float
    assert_eq!(
        eval("(embedding/ref (embedding/list->embedding '(42)) 0)"),
        Value::float(42.0)
    );
}

#[test]
fn test_prompt_system_user_assistant() {
    assert_eq!(
        eval(
            r#"
            (begin
              (define p (prompt (system "be helpful") (user "hello") (assistant "hi")))
              (length (prompt/messages p)))
        "#
        ),
        Value::int(3)
    );
}

#[test]
fn test_prompt_append_preserves_order() {
    // Append two prompts, verify message order
    assert_eq!(
        eval(
            r#"
            (begin
              (define p1 (prompt (user "first")))
              (define p2 (prompt (assistant "second")))
              (define combined (prompt/append p1 p2))
              (define msgs (prompt/messages combined))
              (message/content (car msgs)))
        "#
        ),
        Value::string("first")
    );
}

#[test]
fn test_message_role_error_on_non_message() {
    assert_type_error(r#"(message/role 42)"#);
}

#[test]
fn test_message_content_error_on_non_message() {
    assert_type_error(r#"(message/content "not a msg")"#);
}

#[test]
fn test_prompt_messages_error_on_non_prompt() {
    assert_type_error(r#"(prompt/messages 42)"#);
}

#[test]
fn test_conversation_messages_error_on_non_conversation() {
    assert_type_error(r#"(conversation/messages 42)"#);
}

#[test]
fn test_tool_name_error_on_non_tool() {
    assert_type_error(r#"(tool/name 42)"#);
}

#[test]
fn test_agent_name_error_on_non_agent() {
    assert_type_error(r#"(agent/name "not an agent")"#);
}

#[test]
fn test_conversation_add_message_error_on_non_conversation() {
    assert_type_error(r#"(conversation/add-message 42 :user "hi")"#);
}

#[test]
fn test_conversation_add_message_error_on_bad_role() {
    let err = eval_err(r#"(conversation/add-message (conversation/new) :invalid "hi")"#);
    assert!(err.to_string().contains("unknown role"));
}

#[test]

fn test_stack_trace_nested_functions() {
    let err = eval_err(
        r#"(define (baz z) (+ z "bad"))
           (define (bar y) (begin (baz y) 1))
           (define (foo x) (begin (bar x) 2))
           (foo 1)"#,
    );
    let trace = err.stack_trace().expect("should have stack trace");
    let names: Vec<&str> = trace.0.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names[0], "+");
    assert_eq!(names[1], "baz");
    assert_eq!(names[2], "bar");
    assert_eq!(names[3], "foo");
}

#[test]

fn test_stack_trace_native_fn() {
    let err = eval_err(r#"(define (foo x) (+ x "bad")) (foo 1)"#);
    let trace = err.stack_trace().expect("should have stack trace");
    let names: Vec<&str> = trace.0.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names[0], "+");
    assert_eq!(names[1], "foo");
}

#[test]

fn test_stack_trace_lambda_anonymous() {
    let err = eval_err(r#"((lambda (x) (+ x "bad")) 1)"#);
    let trace = err.stack_trace().expect("should have stack trace");
    let names: Vec<&str> = trace.0.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names[0], "+");
    assert_eq!(names[1], "<lambda>");
}

#[test]

fn test_stack_trace_tco_bounded() {
    let err = eval_err(
        r#"(define (loop n) (if (= n 0) (+ 1 "bad") (loop (- n 1))))
           (loop 100)"#,
    );
    let trace = err.stack_trace().expect("should have stack trace");
    // Should have bounded frames, not 100+ loop frames
    assert!(
        trace.0.len() <= 5,
        "TCO trace should be bounded, got {} frames",
        trace.0.len()
    );
    let names: Vec<&str> = trace.0.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names[0], "+");
    assert_eq!(names[1], "loop");
}

#[test]

fn test_stack_trace_has_spans() {
    let err = eval_err(r#"(define (foo x) (+ x "bad")) (foo 1)"#);
    let trace = err.stack_trace().expect("should have stack trace");
    // At least one frame should have a span (the top-level expression).
    assert!(
        trace.0.iter().any(|f| f.span.is_some()),
        "at least one frame should have a span"
    );
}

#[test]

fn test_stack_trace_in_try_catch() {
    let result = eval(
        r#"(try
             (define (foo x) (+ x "bad"))
             (foo 1)
             (catch e
               (:stack-trace e)))"#,
    );
    // Should be a list of frame maps
    let frames = result.as_list().expect("stack trace should be a list");
    assert!(frames.len() >= 2, "should have at least 2 frames");
    // First frame should be +
    if let Some(first) = frames[0].as_map_rc() {
        assert_eq!(
            first.get(&Value::keyword("name")),
            Some(&Value::string("+"))
        );
    } else {
        panic!("frame should be a map");
    }
}

#[test]

fn test_stack_trace_loaded_file() {
    // Write a file with a function that errors
    let path = temp_path("sema-test-trace.sema");
    std::fs::write(&path, "(define (bad-fn x) (+ x \"oops\"))").unwrap();
    let err = eval_err(&format!(r#"(load "{path}") (bad-fn 1)"#));
    let trace = err.stack_trace().expect("should have stack trace");
    let names: Vec<&str> = trace.0.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names[0], "+");
    assert_eq!(names[1], "bad-fn");
}

#[test]
fn test_tool_slash_name() {
    assert_eq!(
        eval(
            r#"(begin (deftool t1 "desc" {:x {:type :string}} (lambda (x) "ok")) (tool/name t1))"#
        ),
        Value::string("t1")
    );
}

#[test]
fn test_tool_slash_description() {
    assert_eq!(
        eval(
            r#"(begin (deftool t2 "my desc" {:x {:type :string}} (lambda (x) "ok")) (tool/description t2))"#
        ),
        Value::string("my desc")
    );
}

#[test]
fn test_tool_slash_parameters() {
    assert_eq!(
        eval(
            r#"(begin (deftool t3 "desc" {:x {:type :string}} (lambda (x) "ok")) (map? (tool/parameters t3)))"#
        ),
        Value::bool(true)
    );
}

#[test]
fn test_agent_slash_name() {
    assert_eq!(
        eval(r#"(begin (defagent a1 {:system "sys" :tools []}) (agent/name a1))"#),
        Value::string("a1")
    );
}

#[test]
fn test_agent_slash_system() {
    assert_eq!(
        eval(r#"(begin (defagent a2 {:system "you are helpful" :tools []}) (agent/system a2))"#),
        Value::string("you are helpful")
    );
}

#[test]
fn test_agent_slash_tools() {
    assert_eq!(
        eval(r#"(begin (defagent a3 {:system "sys" :tools []}) (length (agent/tools a3)))"#),
        Value::int(0)
    );
}

#[test]
fn test_agent_slash_model() {
    assert_eq!(
        eval(
            r#"(begin (defagent a4 {:system "sys" :tools [] :model "claude-3"}) (agent/model a4))"#
        ),
        Value::string("claude-3")
    );
}

#[test]
fn test_agent_slash_max_turns() {
    assert_eq!(
        eval(
            r#"(begin (defagent a5 {:system "sys" :tools [] :max-turns 5}) (agent/max-turns a5))"#
        ),
        Value::int(5)
    );
}

#[test]
fn test_prompt_slash_messages() {
    assert_eq!(
        eval(r#"(length (prompt/messages (prompt (user "hello") (assistant "hi"))))"#),
        Value::int(2)
    );
}

#[test]
fn test_prompt_slash_append() {
    assert_eq!(
        eval(
            r#"(length (prompt/messages (prompt/append (prompt (user "a")) (prompt (assistant "b")))))"#
        ),
        Value::int(2)
    );
}

#[test]
fn test_prompt_slash_set_system() {
    assert_eq!(
        eval(
            r#"(begin
          (define p (prompt (user "hello")))
          (define p2 (prompt/set-system p "system msg"))
          (length (prompt/messages p2)))"#
        ),
        Value::int(2)
    );
}

#[test]
fn test_message_slash_role() {
    assert_eq!(
        eval(r#"(message/role (message :user "hi"))"#),
        Value::keyword("user")
    );
}

#[test]
fn test_message_slash_content() {
    assert_eq!(
        eval(r#"(message/content (message :user "hello world"))"#),
        Value::string("hello world")
    );
}

#[test]
fn test_legacy_tool_name_alias() {
    assert_eq!(
        eval(
            r#"(begin (deftool t1l "desc" {:x {:type :string}} (lambda (x) "ok")) (tool/name t1l))"#
        ),
        Value::string("t1l")
    );
}

#[test]
fn test_legacy_agent_name_alias() {
    assert_eq!(
        eval(r#"(begin (defagent a1l {:system "sys" :tools []}) (agent/name a1l))"#),
        Value::string("a1l")
    );
}

#[test]
fn test_legacy_prompt_messages_alias() {
    assert_eq!(
        eval(r#"(length (prompt/messages (prompt (user "hello"))))"#),
        Value::int(1)
    );
}

#[test]
fn test_legacy_message_role_alias() {
    assert_eq!(
        eval(r#"(message/role (message :assistant "hi"))"#),
        Value::keyword("assistant")
    );
}

#[test]
fn test_llm_list_providers_empty() {
    // No providers configured, should return empty list
    assert_eq!(eval(r#"(length (llm/list-providers))"#), Value::int(0));
}

#[test]
fn test_llm_current_provider_none() {
    // No provider configured
    assert_eq!(eval(r#"(llm/current-provider)"#), Value::nil());
}

#[test]
fn test_llm_set_budget() {
    // Setting budget should not error
    assert_eq!(
        eval(r#"(begin (llm/set-budget 1.0) (map? (llm/budget-remaining)))"#),
        Value::bool(true)
    );
}

#[test]
fn test_llm_budget_remaining_values() {
    assert_eq!(
        eval(
            r#"(begin
          (llm/set-budget 5.0)
          (define b (llm/budget-remaining))
          (:limit b))"#
        ),
        Value::float(5.0)
    );
}

#[test]
fn test_llm_budget_remaining_no_budget() {
    assert_eq!(
        eval(r#"(begin (llm/clear-budget) (llm/budget-remaining))"#),
        Value::nil()
    );
}

#[test]
fn test_llm_clear_budget() {
    assert_eq!(
        eval(r#"(begin (llm/set-budget 1.0) (llm/clear-budget) (llm/budget-remaining))"#),
        Value::nil()
    );
}

#[test]
fn test_with_budget_restores_outer_scope() {
    let result = eval(
        r#"
        (begin
          (llm/set-budget 10.0)
          (llm/with-budget {:max-cost-usd 1.0} (lambda ()
            (llm/budget-remaining)))
          (llm/budget-remaining))
    "#,
    );

    if let Some(map) = result.as_map_rc() {
        assert_eq!(
            map.get(&Value::keyword("limit")),
            Some(&Value::float(10.0)),
            "outer budget limit should be restored"
        );
        assert_eq!(
            map.get(&Value::keyword("remaining")),
            Some(&Value::float(10.0))
        );
    } else {
        panic!("expected map");
    }
}

#[test]
fn test_with_budget_max_tokens() {
    let result = eval(r#"(llm/with-budget {:max-tokens 5000} (lambda () (llm/budget-remaining)))"#);
    let map = result.as_map_rc().expect("expected map");
    assert_eq!(
        map.get(&Value::keyword("token-limit")),
        Some(&Value::int(5000))
    );
    assert_eq!(
        map.get(&Value::keyword("tokens-spent")),
        Some(&Value::int(0))
    );
    assert_eq!(
        map.get(&Value::keyword("tokens-remaining")),
        Some(&Value::int(5000))
    );
}

#[test]
fn test_with_budget_both_limits() {
    let result = eval(
        r#"(llm/with-budget {:max-cost-usd 0.50 :max-tokens 10000} (lambda () (llm/budget-remaining)))"#,
    );
    let map = result.as_map_rc().expect("expected map");
    assert_eq!(map.get(&Value::keyword("limit")), Some(&Value::float(0.5)));
    assert_eq!(
        map.get(&Value::keyword("token-limit")),
        Some(&Value::int(10000))
    );
}

#[test]
fn test_with_budget_requires_at_least_one_limit() {
    let result = eval_err(r#"(llm/with-budget {} (lambda () 42))"#);
    let msg = format!("{}", result.inner());
    assert!(msg.contains("requires at least"), "got: {msg}");
}

#[test]
fn test_llm_state_isolation_between_interpreters() {
    let interp1 = Interpreter::new();
    interp1.eval_str("(llm/set-budget 5.0)").unwrap();

    let interp2 = Interpreter::new();
    assert_eq!(
        interp2.eval_str("(llm/budget-remaining)").unwrap(),
        Value::nil()
    );
}

#[test]
fn test_llm_set_default_no_provider() {
    // Should error when provider not configured
    let err = eval_err(r#"(llm/set-default :anthropic)"#);
    let msg = format!("{}", err.inner());
    assert!(msg.contains("not configured"), "got: {msg}");
}

// ── ast subcommand (CLI-level tests) ──────────────────────────────

fn sema_cmd() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
}

/// End-to-end: `otel/configure` from Sema code (no env vars) turns tracing on and a
/// user span is written to the configured JSONL file. Runs in a fresh subprocess so the
/// global-provider guard is clean.
#[test]
fn test_otel_configure_from_sema_writes_spans() {
    let path = std::env::temp_dir().join(format!(
        "sema-otel-configure-e2e-{}.jsonl",
        std::process::id()
    ));
    let path_str = path.to_str().unwrap().to_string();
    let _ = std::fs::remove_file(&path);

    let script = format!(
        r#"(let ((on (otel/configure {{:file "{}" :service-name "e2e"}})))
             (otel/span "from-sema" (fn () 42))
             (println on))"#,
        path_str.replace('\\', "\\\\")
    );

    let output = sema_cmd()
        .env_remove("SEMA_OTEL_FILE")
        .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
        .args(["-e", &script])
        .output()
        .expect("failed to run sema");

    assert!(
        output.status.success(),
        "sema exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("#t"),
        "otel/configure should return true when it installs a provider, got: {stdout}"
    );

    let contents = std::fs::read_to_string(&path).expect("jsonl trace file should exist");
    let _ = std::fs::remove_file(&path);
    assert!(
        contents.contains("\"from-sema\""),
        "expected the user span in the trace file, got:\n{contents}"
    );
}

#[test]
fn test_cli_provider_flag_sets_default_provider() {
    let output = sema_cmd()
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("OPENAI_API_KEY", "dummy")
        .args(["--chat-provider", "openai", "-p", "(llm/current-provider)"])
        .output()
        .expect("failed to run sema");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(":name :openai"),
        "expected openai provider, got: {stdout}"
    );
}

#[test]
fn test_cli_model_flag_sets_default_model() {
    let output = sema_cmd()
        .env_remove("ANTHROPIC_API_KEY")
        .env("OPENAI_API_KEY", "dummy")
        .args([
            "--chat-provider",
            "openai",
            "--chat-model",
            "gpt-4o-mini",
            "-p",
            "(llm/current-provider)",
        ])
        .output()
        .expect("failed to run sema");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(":name :openai"),
        "expected openai provider, got: {stdout}"
    );
    assert!(
        stdout.contains(":model \"gpt-4o-mini\""),
        "expected model override, got: {stdout}"
    );
}

#[test]
fn test_cli_chat_model_only_applies_to_default_provider() {
    // When --chat-model is set without --chat-provider, it should only apply
    // to the first (default) provider, not to all providers
    let output = sema_cmd()
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("OPENAI_API_KEY", "dummy")
        .args([
            "--chat-model",
            "claude-haiku-4-5-20251001",
            "-p",
            "(llm/current-provider)",
        ])
        .output()
        .expect("failed to run sema");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Anthropic is first, so it should be the default with the model override
    assert!(
        stdout.contains(":name :anthropic"),
        "expected anthropic as default, got: {stdout}"
    );
    assert!(
        stdout.contains(":model \"claude-haiku-4-5-20251001\""),
        "expected model override on default provider, got: {stdout}"
    );
}

#[test]
fn test_ast_eval_readable() {
    let output = sema_cmd()
        .args(["ast", "-e", "(+ 1 2)"])
        .output()
        .expect("failed to run sema");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("List"), "expected List node: {stdout}");
    assert!(stdout.contains("Symbol +"), "expected Symbol +: {stdout}");
    assert!(stdout.contains("Int 1"), "expected Int 1: {stdout}");
    assert!(stdout.contains("Int 2"), "expected Int 2: {stdout}");
}

#[test]
fn test_ast_eval_json() {
    let output = sema_cmd()
        .args(["ast", "--json", "-e", "(+ 1 2)"])
        .output()
        .expect("failed to run sema");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");
    assert_eq!(json["type"], "list");
    assert_eq!(json["children"][0]["type"], "symbol");
    assert_eq!(json["children"][0]["value"], "+");
    assert_eq!(json["children"][1]["type"], "int");
    assert_eq!(json["children"][1]["value"], 1);
}

#[test]
fn test_ast_multiple_exprs_json() {
    let output = sema_cmd()
        .args(["ast", "--json", "-e", "(+ 1 2) (- 3 4)"])
        .output()
        .expect("failed to run sema");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");
    assert!(json.is_array(), "multiple exprs should produce array");
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[test]
fn test_ast_vector_and_map() {
    let output = sema_cmd()
        .args(["ast", "-e", "[1 :foo]"])
        .output()
        .expect("failed to run sema");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Vector"), "expected Vector: {stdout}");
    assert!(
        stdout.contains("Keyword :foo"),
        "expected Keyword: {stdout}"
    );

    let output = sema_cmd()
        .args(["ast", "--json", "-e", "{:a 1}"])
        .output()
        .expect("failed to run sema");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("invalid JSON");
    assert_eq!(json["type"], "map");
    assert!(json["entries"].is_array());
}

#[test]
fn test_ast_no_input_error() {
    let output = sema_cmd()
        .args(["ast"])
        .output()
        .expect("failed to run sema");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("provide a file or --eval"),
        "expected error message: {stderr}"
    );
}

#[test]
fn test_ast_parse_error() {
    let output = sema_cmd()
        .args(["ast", "-e", "(+ 1"])
        .output()
        .expect("failed to run sema");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Parse error"),
        "expected parse error: {stderr}"
    );
}

#[test]
fn test_string_split_memchr() {
    // Basic split
    assert_eq!(
        eval_to_string(r#"(string/split "a;b;c" ";")"#),
        r#"("a" "b" "c")"#
    );
    // Two-part split (1BRC hot path)
    assert_eq!(
        eval_to_string(r#"(string/split "Berlin;12.3" ";")"#),
        r#"("Berlin" "12.3")"#
    );
    // No match
    assert_eq!(
        eval_to_string(r#"(string/split "hello" ";")"#),
        r#"("hello")"#
    );
    // Multi-char separator
    assert_eq!(
        eval_to_string(r#"(string/split "a::b::c" "::")"#),
        r#"("a" "b" "c")"#
    );
    // Empty parts
    assert_eq!(
        eval_to_string(r#"(string/split "a;;b" ";")"#),
        r#"("a" "" "b")"#
    );
}

#[test]
fn test_hashmap_basic() {
    assert_eq!(
        eval_to_string("(hashmap/get (hashmap/new :a 1 :b 2) :a)"),
        "1"
    );
    assert_eq!(
        eval_to_string("(hashmap/get (hashmap/new :a 1 :b 2) :c)"),
        "nil"
    );
    assert_eq!(
        eval_to_string("(hashmap/get (hashmap/new :a 1) :a 99)"),
        "1"
    );
    assert_eq!(eval_to_string("(hashmap/get (hashmap/new) :a 99)"), "99");
}

#[test]
fn test_hashmap_assoc() {
    assert_eq!(
        eval_to_string("(hashmap/get (hashmap/assoc (hashmap/new) :a 1) :a)"),
        "1"
    );
}

#[test]
fn test_hashmap_to_map() {
    assert_eq!(
        eval_to_string("(hashmap/to-map (hashmap/new :b 2 :a 1))"),
        "{:a 1 :b 2}"
    );
}

#[test]
fn test_hashmap_keys() {
    assert_eq!(
        eval_to_string("(sort (hashmap/keys (hashmap/new :b 2 :a 1)))"),
        "(:a :b)"
    );
}

#[test]
fn test_hashmap_generic_ops() {
    assert_eq!(eval_to_string("(get (hashmap/new :a 1) :a)"), "1");
    assert_eq!(eval_to_string("(get (assoc (hashmap/new) :a 1) :a)"), "1");
    assert_eq!(
        eval_to_string("(sort (keys (hashmap/new :b 2 :a 1)))"),
        "(:a :b)"
    );
    assert_eq!(eval_to_string("(contains? (hashmap/new :a 1) :a)"), "#t");
    assert_eq!(eval_to_string("(count (hashmap/new :a 1 :b 2))"), "2");
    assert_eq!(eval_to_string("(empty? (hashmap/new))"), "#t");
    assert_eq!(eval_to_string("(empty? (hashmap/new :a 1))"), "#f");
    assert_eq!(eval_to_string("(length (hashmap/new :a 1 :b 2))"), "2");
}

#[test]
fn test_car_cdr_compositions() {
    // 2-deep
    assert_eq!(eval("(caar '((1 2) 3))"), Value::int(1));
    assert_eq!(eval("(cadr '(1 2 3))"), Value::int(2));
    assert_eq!(eval_to_string("(cdar '((1 2) 3))"), "(2)");
    assert_eq!(eval_to_string("(cddr '(1 2 3))"), "(3)");
    // 3-deep
    assert_eq!(eval("(caaar '(((1 2) 3) 4))"), Value::int(1));
    assert_eq!(eval("(caadr '(1 (2 3) 4))"), Value::int(2));
    assert_eq!(eval("(cadar '((1 2 3) 4))"), Value::int(2));
    assert_eq!(eval("(caddr '(1 2 3 4))"), Value::int(3));
    assert_eq!(eval_to_string("(cdaar '(((1 2 3)) 4))"), "(2 3)");
    assert_eq!(eval_to_string("(cdadr '(1 (2 3 4)))"), "(3 4)");
    assert_eq!(eval_to_string("(cddar '((1 2 3) 4))"), "(3)");
    assert_eq!(eval_to_string("(cdddr '(1 2 3 4))"), "(4)");
}

#[test]
fn test_assoc_alist() {
    assert_eq!(
        eval_to_string(r#"(assoc "b" '(("a" 1) ("b" 2) ("c" 3)))"#),
        r#"("b" 2)"#
    );
    assert_eq!(
        eval(r#"(assoc "z" '(("a" 1) ("b" 2)))"#),
        Value::bool(false)
    );
    assert_eq!(eval_to_string("(assoc 2 '((1 a) (2 b) (3 c)))"), "(2 b)");
}

#[test]
fn test_assq() {
    assert_eq!(
        eval_to_string("(assq :name '((:name \"Alice\") (:age 30)))"),
        r#"(:name "Alice")"#
    );
    assert_eq!(eval("(assq :missing '((:a 1) (:b 2)))"), Value::bool(false));
}

#[test]
fn test_assv() {
    assert_eq!(eval_to_string("(assv 42 '((1 a) (42 b) (3 c)))"), "(42 b)");
    assert_eq!(eval("(assv 99 '((1 a) (2 b)))"), Value::bool(false));
}

#[test]
fn test_do_loop_basic() {
    // Sum 1..10
    assert_eq!(
        eval("(do ((i 0 (+ i 1)) (sum 0 (+ sum i))) ((= i 10) sum))"),
        Value::int(45)
    );
}

#[test]
fn test_do_loop_factorial() {
    assert_eq!(
        eval("(do ((i 1 (+ i 1)) (acc 1 (* acc i))) ((> i 5) acc))"),
        Value::int(120)
    );
}

#[test]
fn test_do_loop_with_body() {
    // Body executes for side effects; test clause returns result
    assert_eq!(
        eval("(begin (define count 0) (do ((i 0 (+ i 1))) ((= i 3) count) (set! count (+ count 1))))"),
        Value::int(3)
    );
}

#[test]
fn test_do_loop_no_step() {
    // Variable without step expr stays constant
    assert_eq!(
        eval("(do ((x 10) (i 0 (+ i 1))) ((= i 3) x))"),
        Value::int(10)
    );
}

#[test]
fn test_do_begin_still_works() {
    assert_eq!(eval("(begin 1 2 3)"), Value::int(3));
    assert_eq!(
        eval("(begin (define x 10) (define y 20) (+ x y))"),
        Value::int(30)
    );
}

#[test]
fn test_char_literals() {
    assert_eq!(eval(r"#\a"), Value::char('a'));
    assert_eq!(eval(r"#\Z"), Value::char('Z'));
    assert_eq!(eval(r"#\space"), Value::char(' '));
    assert_eq!(eval(r"#\newline"), Value::char('\n'));
    assert_eq!(eval(r"#\tab"), Value::char('\t'));
}

#[test]
fn test_char_predicate() {
    assert_eq!(eval(r"(char? #\a)"), Value::bool(true));
    assert_eq!(eval(r#"(char? "a")"#), Value::bool(false));
    assert_eq!(eval("(char? 42)"), Value::bool(false));
}

#[test]
fn test_char_conversions() {
    assert_eq!(eval(r"(char->integer #\a)"), Value::int(97));
    assert_eq!(eval(r"(char->integer #\A)"), Value::int(65));
    assert_eq!(eval("(integer->char 97)"), Value::char('a'));
    assert_eq!(eval(r#"(char->string #\x)"#), Value::string("x"));
    assert_eq!(eval(r#"(string->char "z")"#), Value::char('z'));
}

#[test]
fn test_char_predicates() {
    assert_eq!(eval(r"(char-alphabetic? #\a)"), Value::bool(true));
    assert_eq!(eval(r"(char-alphabetic? #\1)"), Value::bool(false));
    assert_eq!(eval(r"(char-numeric? #\5)"), Value::bool(true));
    assert_eq!(eval(r"(char-numeric? #\a)"), Value::bool(false));
    assert_eq!(eval(r"(char-whitespace? #\space)"), Value::bool(true));
    assert_eq!(eval(r"(char-whitespace? #\a)"), Value::bool(false));
    assert_eq!(eval(r"(char-upper-case? #\A)"), Value::bool(true));
    assert_eq!(eval(r"(char-upper-case? #\a)"), Value::bool(false));
    assert_eq!(eval(r"(char-lower-case? #\a)"), Value::bool(true));
    assert_eq!(eval(r"(char-lower-case? #\A)"), Value::bool(false));
}

#[test]
fn test_char_case() {
    assert_eq!(eval(r"(char-upcase #\a)"), Value::char('A'));
    assert_eq!(eval(r"(char-downcase #\A)"), Value::char('a'));
    assert_eq!(eval(r"(char-upcase #\1)"), Value::char('1'));
}

#[test]
fn test_string_ref_returns_char() {
    assert_eq!(eval(r#"(char? (string-ref "hello" 0))"#), Value::bool(true));
    assert_eq!(eval(r#"(string-ref "abc" 1)"#), Value::char('b'));
}

#[test]
fn test_string_to_list_chars() {
    assert_eq!(
        eval_to_string(r#"(string->list "abc")"#),
        "(#\\a #\\b #\\c)"
    );
    assert_eq!(eval_to_string(r#"(string->list "")"#), "()");
}

#[test]
fn test_list_to_string() {
    assert_eq!(
        eval(r#"(list->string (list #\h #\i))"#),
        Value::string("hi")
    );
    assert_eq!(eval(r#"(list->string '())"#), Value::string(""));
}

#[test]
fn test_delay_force_basic() {
    assert_eq!(eval("(force (delay (+ 1 2)))"), Value::int(3));
    assert_eq!(eval("(force (delay 42))"), Value::int(42));
}

#[test]
fn test_delay_is_promise() {
    assert_eq!(eval("(promise? (delay 1))"), Value::bool(true));
    assert_eq!(eval("(promise? 42)"), Value::bool(false));
    assert_eq!(eval("(promise? (list 1))"), Value::bool(false));
}

#[test]
fn test_delay_memoization() {
    // Body evaluated only once — counter only increments once
    assert_eq!(
        eval("(begin (define counter 0) (define p (delay (begin (set! counter (+ counter 1)) counter))) (force p) (force p) counter)"),
        Value::int(1)
    );
}

#[test]
fn test_force_non_promise() {
    // Calling `force` on a non-promise is now an error (D4): previously it
    // silently returned the argument as-is, but this hid bugs and
    // diverged from the VM intrinsic. Non-promises are now rejected.
    let err = eval_err("(force 42)");
    assert!(
        format!("{err}").contains("thunk"),
        "expected type-error mentioning 'thunk', got: {err}"
    );
    let err = eval_err(r#"(force "hello")"#);
    assert!(
        format!("{err}").contains("thunk"),
        "expected type-error mentioning 'thunk', got: {err}"
    );
}

#[test]
fn test_promise_forced_predicate() {
    assert_eq!(
        eval("(begin (define p (delay 1)) (promise-forced? p))"),
        Value::bool(false)
    );
    assert_eq!(
        eval("(begin (define p (delay 1)) (force p) (promise-forced? p))"),
        Value::bool(true)
    );
}

#[test]
fn test_char_comparison_predicates() {
    assert_eq!(eval("(char=? #\\a #\\a)"), Value::bool(true));
    assert_eq!(eval("(char=? #\\a #\\b)"), Value::bool(false));
    assert_eq!(eval("(char<? #\\a #\\b)"), Value::bool(true));
    assert_eq!(eval("(char<? #\\b #\\a)"), Value::bool(false));
    assert_eq!(eval("(char>? #\\b #\\a)"), Value::bool(true));
    assert_eq!(eval("(char>? #\\a #\\b)"), Value::bool(false));
    assert_eq!(eval("(char<=? #\\a #\\a)"), Value::bool(true));
    assert_eq!(eval("(char<=? #\\a #\\b)"), Value::bool(true));
    assert_eq!(eval("(char<=? #\\b #\\a)"), Value::bool(false));
    assert_eq!(eval("(char>=? #\\b #\\a)"), Value::bool(true));
    assert_eq!(eval("(char>=? #\\a #\\a)"), Value::bool(true));
    assert_eq!(eval("(char>=? #\\a #\\b)"), Value::bool(false));
}

#[test]
fn test_char_ci_comparison_predicates() {
    assert_eq!(eval("(char-ci=? #\\A #\\a)"), Value::bool(true));
    assert_eq!(eval("(char-ci=? #\\a #\\A)"), Value::bool(true));
    assert_eq!(eval("(char-ci=? #\\a #\\b)"), Value::bool(false));
    assert_eq!(eval("(char-ci<? #\\A #\\b)"), Value::bool(true));
    assert_eq!(eval("(char-ci<? #\\a #\\B)"), Value::bool(true));
    assert_eq!(eval("(char-ci>? #\\b #\\A)"), Value::bool(true));
    assert_eq!(eval("(char-ci<=? #\\A #\\a)"), Value::bool(true));
    assert_eq!(eval("(char-ci>=? #\\a #\\A)"), Value::bool(true));
}

#[test]
fn test_define_record_type_basic() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define-record-type point
            (make-point x y)
            point?
            (x point-x)
            (y point-y))
        (define p (make-point 3 4))
        (list (point? p) (point-x p) (point-y p))
    ",
        )
        .unwrap();
    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::int(3), Value::int(4)])
    );
}

#[test]
fn test_define_record_type_predicate_false() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define-record-type point
            (make-point x y)
            point?
            (x point-x)
            (y point-y))
        (point? 42)
    ",
        )
        .unwrap();
    assert_eq!(result, Value::bool(false));
}

#[test]
fn test_define_record_type_equality() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define-record-type point
            (make-point x y)
            point?
            (x point-x)
            (y point-y))
        (equal? (make-point 1 2) (make-point 1 2))
    ",
        )
        .unwrap();
    assert_eq!(result, Value::bool(true));
}

#[test]
fn test_define_record_type_type_function() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define-record-type point
            (make-point x y)
            point?
            (x point-x)
            (y point-y))
        (type (make-point 1 2))
    ",
        )
        .unwrap();
    assert_eq!(result, Value::keyword("point"));
}

#[test]
fn test_define_record_type_record_predicate() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define-record-type point
            (make-point x y)
            point?
            (x point-x)
            (y point-y))
        (list (record? (make-point 1 2)) (record? 42))
    ",
        )
        .unwrap();
    assert_eq!(
        result,
        Value::list(vec![Value::bool(true), Value::bool(false)])
    );
}

#[test]
fn test_define_record_type_mutator_ignored() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define-record-type point
            (make-point x y)
            point?
            (x point-x set-point-x!)
            (y point-y set-point-y!))
        (point-x (make-point 7 8))
    ",
        )
        .unwrap();
    assert_eq!(result, Value::int(7));
}

#[test]
fn test_define_record_type_multiple_types() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define-record-type point (make-point x y) point? (x point-x) (y point-y))
        (define-record-type color (make-color r g b) color? (r color-r) (g color-g) (b color-b))
        (list (point? (make-point 1 2)) (color? (make-point 1 2))
              (color? (make-color 255 0 0)) (point? (make-color 255 0 0)))
    ",
        )
        .unwrap();
    assert_eq!(
        result,
        Value::list(vec![
            Value::bool(true),
            Value::bool(false),
            Value::bool(true),
            Value::bool(false),
        ])
    );
}

#[test]
fn test_bytevector_constructors() {
    assert_eq!(eval("(bytevector 1 2 3)"), Value::bytevector(vec![1, 2, 3]));
    assert_eq!(
        eval("(make-bytevector 3 7)"),
        Value::bytevector(vec![7, 7, 7])
    );
    assert_eq!(
        eval("(make-bytevector 4)"),
        Value::bytevector(vec![0, 0, 0, 0])
    );
}

#[test]
fn test_bytevector_length() {
    assert_eq!(eval("(bytevector-length #u8(1 2 3))"), Value::int(3));
    assert_eq!(eval("(bytevector-length #u8())"), Value::int(0));
}

#[test]
fn test_bytevector_u8_ref() {
    assert_eq!(eval("(bytevector-u8-ref #u8(10 20 30) 0)"), Value::int(10));
    assert_eq!(eval("(bytevector-u8-ref #u8(10 20 30) 2)"), Value::int(30));
}

#[test]
fn test_bytevector_u8_set() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            "
        (define a #u8(1 2 3))
        (define b (bytevector-u8-set! a 0 9))
        (list a b)
    ",
        )
        .unwrap();
    assert_eq!(
        result,
        Value::list(vec![
            Value::bytevector(vec![1, 2, 3]),
            Value::bytevector(vec![9, 2, 3]),
        ])
    );
}

#[test]
fn test_bytevector_copy() {
    assert_eq!(
        eval("(bytevector-copy #u8(1 2 3 4 5) 1 3)"),
        Value::bytevector(vec![2, 3])
    );
    assert_eq!(
        eval("(bytevector-copy #u8(1 2 3))"),
        Value::bytevector(vec![1, 2, 3])
    );
}

#[test]
fn test_bytevector_append() {
    assert_eq!(
        eval("(bytevector-append #u8(1 2) #u8(3 4))"),
        Value::bytevector(vec![1, 2, 3, 4])
    );
    assert_eq!(
        eval("(bytevector-append #u8(1) #u8(2) #u8(3))"),
        Value::bytevector(vec![1, 2, 3])
    );
}

#[test]
fn test_bytevector_list_conversion() {
    assert_eq!(
        eval("(bytevector->list #u8(65 66 67))"),
        Value::list(vec![Value::int(65), Value::int(66), Value::int(67)])
    );
    assert_eq!(
        eval("(list->bytevector (list 1 2 3))"),
        Value::bytevector(vec![1, 2, 3])
    );
}

#[test]
fn test_bytevector_utf8_conversion() {
    assert_eq!(eval_to_string("(utf8->string #u8(104 105))"), "\"hi\"");
    assert_eq!(
        eval("(string->utf8 \"hi\")"),
        Value::bytevector(vec![104, 105])
    );
}

#[test]
fn test_bytevector_predicate() {
    assert_eq!(eval("(bytevector? #u8(1 2))"), Value::bool(true));
    assert_eq!(eval("(bytevector? 42)"), Value::bool(false));
    assert_eq!(eval("(bytevector? (list 1 2))"), Value::bool(false));
}

#[test]
fn test_bytevector_display() {
    assert_eq!(eval_to_string("#u8(1 2 3)"), "#u8(1 2 3)");
    assert_eq!(eval_to_string("#u8()"), "#u8()");
}

#[test]
fn test_truncate() {
    // Exactness-preserving (R7RS): a float argument truncates to a float.
    assert_eq!(eval("(truncate 3.9)"), Value::float(3.0));
    assert_eq!(eval("(truncate -3.9)"), Value::float(-3.0));
    assert_eq!(eval("(truncate 5)"), Value::int(5));
}

#[test]
fn test_scheme_aliases() {
    assert_eq!(eval("(modulo 17 5)"), Value::int(2));
    assert_eq!(eval("(expt 2 10)"), Value::int(1024));
    assert_eq!(eval("(ceiling 3.2)"), Value::float(4.0)); // exactness-preserving
}

#[test]
fn test_math_pow_namespaced() {
    assert_float_eq("(math/pow 2.0 10.0)", 1024.0);
    assert_eq!(eval("(math/pow 2 3)"), Value::int(8));
}

#[test]
fn test_math_hyperbolic() {
    assert_float_eq("(math/sinh 0)", 0.0);
    assert_float_eq("(math/cosh 0)", 1.0);
    assert_float_eq("(math/tanh 0)", 0.0);
}

#[test]
fn test_math_angle_conversion() {
    if let Some(f) = eval("(math/degrees->radians 180)").as_float() {
        assert!((f - std::f64::consts::PI).abs() < 1e-10);
    } else {
        panic!("expected float");
    }
    if let Some(f) = eval("(math/radians->degrees pi)").as_float() {
        assert!((f - 180.0).abs() < 1e-10);
    } else {
        panic!("expected float");
    }
}

#[test]
fn test_math_lerp() {
    assert_float_eq("(math/lerp 0.0 10.0 0.5)", 5.0);
    assert_float_eq("(math/lerp 0.0 10.0 0.0)", 0.0);
    assert_float_eq("(math/lerp 0.0 10.0 1.0)", 10.0);
}

#[test]
fn test_math_map_range() {
    assert_eq!(
        eval("(math/map-range 5.0 0.0 10.0 0.0 100.0)"),
        Value::float(50.0)
    );
    assert_eq!(
        eval("(math/map-range 0.0 0.0 10.0 0.0 100.0)"),
        Value::float(0.0)
    );
}

#[test]
fn test_math_special_values() {
    assert_eq!(eval("(math/nan? math/nan)"), Value::bool(true));
    assert_eq!(eval("(math/nan? 1.0)"), Value::bool(false));
    assert_eq!(eval("(math/infinite? math/infinity)"), Value::bool(true));
    assert_eq!(eval("(math/infinite? 1.0)"), Value::bool(false));
}

#[test]
fn test_even_odd_predicates() {
    assert_eq!(eval("(even? 4)"), Value::bool(true));
    assert_eq!(eval("(even? 3)"), Value::bool(false));
    assert_eq!(eval("(odd? 3)"), Value::bool(true));
    assert_eq!(eval("(odd? 4)"), Value::bool(false));
    assert_eq!(eval("(even? 0)"), Value::bool(true));
}

#[test]
fn test_take_while() {
    assert_eq!(
        eval_to_string("(take-while (lambda (x) (< x 4)) (list 1 2 3 4 5))"),
        "(1 2 3)"
    );
    assert_eq!(
        eval_to_string("(take-while (lambda (x) (< x 1)) (list 1 2 3))"),
        "()"
    );
}

#[test]
fn test_drop_while() {
    assert_eq!(
        eval_to_string("(drop-while (lambda (x) (< x 4)) (list 1 2 3 4 5))"),
        "(4 5)"
    );
    assert_eq!(
        eval_to_string("(drop-while (lambda (x) (< x 10)) (list 1 2 3))"),
        "()"
    );
}

#[test]
fn test_list_dedupe() {
    assert_eq!(
        eval_to_string("(list/dedupe (list 1 1 2 2 3 1 1))"),
        "(1 2 3 1)"
    );
    assert_eq!(eval_to_string("(list/dedupe (list 1 2 3))"), "(1 2 3)");
    assert_eq!(eval_to_string("(list/dedupe (list))"), "()");
}

#[test]
fn test_flat_map() {
    assert_eq!(
        eval_to_string("(flat-map (lambda (x) (list x (* x 10))) (list 1 2 3))"),
        "(1 10 2 20 3 30)"
    );
    assert_eq!(
        eval_to_string("(flat-map (lambda (x) (list)) (list 1 2 3))"),
        "()"
    );
}

#[test]
fn test_sys_home_dir() {
    let result = eval("(sys/home-dir)");
    assert!(result.is_string());
    if let Some(s) = result.as_str() {
        assert!(!s.is_empty());
    }
}

#[test]
fn test_sys_temp_dir() {
    let result = eval("(sys/temp-dir)");
    assert!(result.is_string());
    if let Some(s) = result.as_str() {
        assert!(!s.is_empty());
    }
}

#[test]
fn test_sys_hostname() {
    let result = eval("(sys/hostname)");
    assert!(result.is_string());
    let s = result.as_str().expect("hostname should be a string");
    assert!(!s.is_empty(), "hostname should be non-empty, got: {s:?}");
}

#[test]
fn test_sys_user() {
    let result = eval("(sys/user)");
    assert!(result.is_string());
    if let Some(s) = result.as_str() {
        assert!(!s.is_empty());
    }
}

#[test]
fn test_string_last_index_of() {
    assert_eq!(
        eval(r#"(string/last-index-of "abcabc" "bc")"#),
        Value::int(4)
    );
    assert_eq!(
        eval(r#"(string/last-index-of "hello" "xyz")"#),
        Value::nil()
    );
    assert_eq!(eval(r#"(string/last-index-of "aaa" "a")"#), Value::int(2));
}

#[test]
fn test_string_reverse() {
    assert_eq!(eval(r#"(string/reverse "hello")"#), Value::string("olleh"));
    assert_eq!(eval(r#"(string/reverse "")"#), Value::string(""));
    assert_eq!(eval(r#"(string/reverse "a")"#), Value::string("a"));
}

#[test]
fn test_string_empty() {
    assert_eq!(eval(r#"(string/empty? "")"#), Value::bool(true));
    assert_eq!(eval(r#"(string/empty? "hello")"#), Value::bool(false));
    assert_eq!(eval(r#"(string/empty? " ")"#), Value::bool(false));
}

#[test]
fn test_string_capitalize() {
    assert_eq!(
        eval(r#"(string/capitalize "hello")"#),
        Value::string("Hello")
    );
    assert_eq!(
        eval(r#"(string/capitalize "HELLO")"#),
        Value::string("Hello")
    );
    assert_eq!(eval(r#"(string/capitalize "")"#), Value::string(""));
    assert_eq!(eval(r#"(string/capitalize "a")"#), Value::string("A"));
}

#[test]
fn test_string_title_case() {
    assert_eq!(
        eval(r#"(string/title-case "hello world")"#),
        Value::string("Hello World")
    );
    assert_eq!(
        eval(r#"(string/title-case "foo bar baz")"#),
        Value::string("Foo Bar Baz")
    );
    assert_eq!(eval(r#"(string/title-case "")"#), Value::string(""));
}

// IO: print-error, println-error

#[test]
fn test_print_error() {
    assert_eq!(eval(r#"(print-error "hello")"#), Value::nil());
    assert_eq!(eval(r#"(print-error "a" "b" "c")"#), Value::nil());
    assert_eq!(eval(r#"(print-error 42)"#), Value::nil());
}

#[test]
fn test_println_error() {
    assert_eq!(eval(r#"(println-error "hello")"#), Value::nil());
    assert_eq!(eval(r#"(println-error "a" "b" "c")"#), Value::nil());
    assert_eq!(eval(r#"(println-error)"#), Value::nil());
}

// System: sys/interactive?

#[test]
fn test_sys_interactive() {
    // Result depends on whether stdin is a TTY; just verify it returns a boolean
    let result = eval("(sys/interactive?)");
    assert!(result.is_bool());
    assert_eq!(eval("(boolean? (sys/interactive?))"), Value::bool(true));
}

#[test]
fn test_sys_tty() {
    // In test/CI context may or may not have a TTY; just verify it returns string or nil
    let result = eval("(sys/tty)");
    assert!(result.is_string() || result.is_nil());
}

#[test]
fn test_sys_pid() {
    let result = eval("(sys/pid)");
    let pid = result.as_int().expect("expected int");
    assert!(pid > 0);
}

#[test]
fn test_sys_arch() {
    let result = eval("(sys/arch)");
    assert!(result.is_string());
    assert_eq!(eval("(string? (sys/arch))"), Value::bool(true));
}

#[test]
fn test_sys_os() {
    let result = eval("(sys/os)");
    assert!(result.is_string());
    assert_eq!(eval("(string? (sys/os))"), Value::bool(true));
}

#[test]
#[cfg(unix)]
fn test_sys_which() {
    // "sh" should exist on any unix system
    let result = eval(r#"(sys/which "sh")"#);
    assert!(result.is_string());
    // non-existent binary returns nil
    assert_eq!(
        eval(r#"(sys/which "this-binary-does-not-exist-xyz")"#),
        Value::nil()
    );
}

#[test]
fn test_sys_elapsed() {
    let result = eval("(sys/elapsed)");
    let ns = result.as_int().expect("expected int");
    assert!(ns >= 0);
    // Two calls should be monotonically increasing
    assert_eq!(
        eval("(let ((a (sys/elapsed)) (b (sys/elapsed))) (<= a b))"),
        Value::bool(true)
    );
}

// List: list/shuffle, list/split-at, list/take-while, list/drop-while,
//       list/sum, list/min, list/max, list/pick, list/repeat, iota

#[test]
fn test_list_shuffle() {
    // Shuffle returns a list of same length with same elements
    let result = eval("(length (list/shuffle (list 1 2 3 4 5)))");
    assert_eq!(result, Value::int(5));
    // Shuffle of empty list is empty
    assert_eq!(eval("(list/shuffle '())"), Value::list(vec![]));
}

#[test]
fn test_list_split_at() {
    assert_eq!(
        eval("(list/split-at (list 1 2 3 4 5) 3)"),
        Value::list(vec![
            Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
            Value::list(vec![Value::int(4), Value::int(5)]),
        ])
    );
    assert_eq!(
        eval("(list/split-at (list 1 2 3) 0)"),
        Value::list(vec![
            Value::list(vec![]),
            Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
        ])
    );
    assert_eq!(
        eval("(list/split-at (list 1 2 3) 5)"),
        Value::list(vec![
            Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]),
            Value::list(vec![]),
        ])
    );
}

#[test]
fn test_list_take_while() {
    assert_eq!(
        eval("(list/take-while (lambda (x) (< x 4)) (list 1 2 3 4 5))"),
        Value::list(vec![Value::int(1), Value::int(2), Value::int(3)])
    );
    assert_eq!(
        eval("(list/take-while (lambda (x) (< x 1)) (list 1 2 3))"),
        Value::list(vec![])
    );
    assert_eq!(
        eval("(list/take-while (lambda (x) #t) (list 1 2 3))"),
        Value::list(vec![Value::int(1), Value::int(2), Value::int(3)])
    );
}

#[test]
fn test_list_drop_while() {
    assert_eq!(
        eval("(list/drop-while (lambda (x) (< x 4)) (list 1 2 3 4 5))"),
        Value::list(vec![Value::int(4), Value::int(5)])
    );
    assert_eq!(
        eval("(list/drop-while (lambda (x) (< x 1)) (list 1 2 3))"),
        Value::list(vec![Value::int(1), Value::int(2), Value::int(3)])
    );
    assert_eq!(
        eval("(list/drop-while (lambda (x) #t) (list 1 2 3))"),
        Value::list(vec![])
    );
}

#[test]
fn test_list_sum() {
    assert_eq!(eval("(list/sum (list 1 2 3 4 5))"), Value::int(15));
    assert_eq!(eval("(list/sum (list 1.0 2.0 3.0))"), Value::float(6.0));
    assert_eq!(eval("(list/sum (list 1 2.0 3))"), Value::float(6.0));
    assert_eq!(eval("(list/sum '())"), Value::int(0));
}

#[test]
fn test_list_min() {
    assert_eq!(eval("(list/min (list 3 1 4 1 5))"), Value::int(1));
    assert_eq!(eval("(list/min (list 3.0 1.5 2.0))"), Value::float(1.5));
    assert_eq!(eval("(list/min (list 42))"), Value::int(42));
}

#[test]
fn test_list_max() {
    assert_eq!(eval("(list/max (list 3 1 4 1 5))"), Value::int(5));
    assert_eq!(eval("(list/max (list 3.0 1.5 2.0))"), Value::float(3.0));
    assert_eq!(eval("(list/max (list 42))"), Value::int(42));
}

#[test]
fn test_list_pick() {
    // Pick returns an element from the list
    let result = eval("(list/pick (list 1 2 3 4 5))");
    let n = result.as_int().expect("expected int");
    assert!((1..=5).contains(&n));
}

#[test]
fn test_list_repeat() {
    assert_eq!(
        eval("(list/repeat 3 0)"),
        Value::list(vec![Value::int(0), Value::int(0), Value::int(0)])
    );
    assert_eq!(
        eval(r#"(list/repeat 2 "x")"#),
        Value::list(vec![Value::string("x"), Value::string("x")])
    );
    assert_eq!(eval("(list/repeat 0 1)"), Value::list(vec![]));
    // make-list alias
    assert_eq!(
        eval("(make-list 3 0)"),
        Value::list(vec![Value::int(0), Value::int(0), Value::int(0)])
    );
}

#[test]
fn test_iota() {
    assert_eq!(
        eval("(iota 5)"),
        Value::list(vec![
            Value::int(0),
            Value::int(1),
            Value::int(2),
            Value::int(3),
            Value::int(4)
        ])
    );
    assert_eq!(
        eval("(iota 3 10)"),
        Value::list(vec![Value::int(10), Value::int(11), Value::int(12)])
    );
    assert_eq!(
        eval("(iota 4 0 2)"),
        Value::list(vec![
            Value::int(0),
            Value::int(2),
            Value::int(4),
            Value::int(6)
        ])
    );
    assert_eq!(eval("(iota 0)"), Value::list(vec![]));
}

// String: string/map

#[test]
fn test_string_map() {
    assert_eq!(
        eval(r#"(string/map char-upcase "hello")"#),
        Value::string("HELLO")
    );
    assert_eq!(
        eval(r#"(string/map (lambda (c) c) "abc")"#),
        Value::string("abc")
    );
    assert_eq!(eval(r#"(string/map char-upcase "")"#), Value::string(""));
}

// Terminal: term/ color and style functions

#[test]
fn test_term_bold() {
    assert_eq!(
        eval(r#"(term/bold "hello")"#),
        Value::string("\x1b[1mhello\x1b[0m")
    );
}

#[test]
fn test_term_dim() {
    assert_eq!(
        eval(r#"(term/dim "text")"#),
        Value::string("\x1b[2mtext\x1b[0m")
    );
}

#[test]
fn test_term_colors() {
    assert_eq!(
        eval(r#"(term/red "error")"#),
        Value::string("\x1b[31merror\x1b[0m")
    );
    assert_eq!(
        eval(r#"(term/green "ok")"#),
        Value::string("\x1b[32mok\x1b[0m")
    );
    assert_eq!(
        eval(r#"(term/cyan "info")"#),
        Value::string("\x1b[36minfo\x1b[0m")
    );
    assert_eq!(
        eval(r#"(term/gray "muted")"#),
        Value::string("\x1b[90mmuted\x1b[0m")
    );
}

#[test]
fn test_term_style_compound() {
    assert_eq!(
        eval(r#"(term/style "text" :bold :red)"#),
        Value::string("\x1b[1;31mtext\x1b[0m")
    );
    assert_eq!(
        eval(r#"(term/style "ok" :bold :green)"#),
        Value::string("\x1b[1;32mok\x1b[0m")
    );
}

#[test]
fn test_term_style_no_keywords() {
    // term/style with just text and no keywords returns plain text
    assert_eq!(eval(r#"(term/style "plain")"#), Value::string("plain"));
}

#[test]
fn test_term_strip() {
    assert_eq!(
        eval(r#"(term/strip (term/bold "hello"))"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(term/strip (term/style "text" :bold :red))"#),
        Value::string("text")
    );
    assert_eq!(
        eval(r#"(term/strip "no ansi here")"#),
        Value::string("no ansi here")
    );
}

#[test]
fn test_term_rgb() {
    assert_eq!(
        eval(r#"(term/rgb "hi" 255 100 0)"#),
        Value::string("\x1b[38;2;255;100;0mhi\x1b[0m")
    );
}

#[test]
fn test_term_strip_rgb() {
    assert_eq!(
        eval(r#"(term/strip (term/rgb "hi" 255 100 0))"#),
        Value::string("hi")
    );
}

#[test]
fn test_term_spinner_start_stop() {
    // Spinner start returns an integer ID, stop returns nil
    assert_eq!(
        eval(
            r#"(let ((id (term/spinner-start "test")))
                   (term/spinner-stop id))"#
        ),
        Value::nil()
    );
}

#[test]
fn test_term_spinner_update() {
    assert_eq!(
        eval(
            r#"(let ((id (term/spinner-start "initial")))
                   (term/spinner-update id "updated")
                   (term/spinner-stop id))"#
        ),
        Value::nil()
    );
}

// Regression tests: deftool params stored as BTreeMap (alphabetical keys).
// Lambda handlers must receive args in declaration order, not alphabetical.
// The actual json_args_to_sema ordering fix is tested via unit tests in
// sema-llm/src/builtins.rs (test_execute_tool_call_arg_ordering, etc.).
// These integration tests verify that deftool creates a well-formed tool
// definition with the expected name, description, and parameter map.

#[test]
fn test_deftool_lambda_param_order_not_alphabetical() {
    // Params :path/:content — alphabetically content < path, but lambda declares (path content).
    // Verify the tool definition is created with correct metadata.
    assert_eq!(
        eval(
            r#"(begin
              (deftool write-file
                "Write content to a file"
                {:path {:type :string :description "File path"}
                 :content {:type :string :description "File content"}}
                (lambda (path content) (list path content)))
              (tool/name write-file))"#,
        ),
        Value::string("write-file")
    );
    assert_eq!(
        eval(
            r#"(begin
              (deftool write-file
                "Write content to a file"
                {:path {:type :string :description "File path"}
                 :content {:type :string :description "File content"}}
                (lambda (path content) (list path content)))
              (tool/description write-file))"#,
        ),
        Value::string("Write content to a file")
    );
    // Param keys are BTreeMap-sorted (alphabetical) — this is the root cause
    // of the ordering bug that json_args_to_sema fixes
    // Note: output order relies on BTreeMap sorted iteration
    assert_eq!(
        eval_to_string(
            r#"(begin
              (deftool write-file
                "Write content to a file"
                {:path {:type :string :description "File path"}
                 :content {:type :string :description "File content"}}
                (lambda (path content) (list path content)))
              (keys (tool/parameters write-file)))"#,
        ),
        "(:content :path)"
    );
}

#[test]
fn test_deftool_params_are_map() {
    // Verify tool/parameters returns a map (BTreeMap), confirming why ordering matters.
    assert_eq!(
        eval(
            r#"(begin
              (deftool t1 "desc"
                {:zebra {:type :string} :apple {:type :string}}
                (lambda (zebra apple) "ok"))
              (map? (tool/parameters t1)))"#,
        ),
        Value::bool(true)
    );
}

#[test]
fn test_deftool_param_keys_sorted_alphabetically() {
    // Confirm that tool parameter map keys come out alphabetically (BTreeMap),
    // demonstrating the ordering bug this fix addresses.
    let result = eval(
        r#"(begin
          (deftool t2 "desc"
            {:zebra {:type :string} :apple {:type :string}}
            (lambda (zebra apple) "ok"))
          (keys (tool/parameters t2)))"#,
    );
    // BTreeMap sorts: :apple before :zebra — opposite of declaration order
    assert_eq!(format!("{}", result), "(:apple :zebra)");
}

#[test]
fn test_deftool_three_params_ordering() {
    // Verify that deftool with 3 params creates a tool with correct structure.
    // The actual execute_tool_call ordering fix is tested in unit tests
    // (sema-llm/src/builtins.rs::test_execute_tool_call_arg_ordering).
    assert_eq!(
        eval(
            r#"(begin
              (deftool multi-tool "test"
                {:c_third {:type :string}
                 :a_first {:type :string}
                 :b_second {:type :string}}
                (lambda (c_third a_first b_second)
                  (string-append c_third "-" a_first "-" b_second)))
              (tool/name multi-tool))"#,
        ),
        Value::string("multi-tool")
    );
    // BTreeMap sorts: a_first < b_second < c_third — opposite of declaration order
    // Note: output order relies on BTreeMap sorted iteration
    assert_eq!(
        eval_to_string(
            r#"(begin
              (deftool multi-tool "test"
                {:c_third {:type :string}
                 :a_first {:type :string}
                 :b_second {:type :string}}
                (lambda (c_third a_first b_second)
                  (string-append c_third "-" a_first "-" b_second)))
              (keys (tool/parameters multi-tool)))"#,
        ),
        "(:a_first :b_second :c_third)"
    );
}

#[test]
fn test_unicode_string_length() {
    // string-length should count characters, not bytes
    assert_eq!(eval(r#"(string-length "hello")"#), Value::int(5));
    assert_eq!(eval(r#"(string-length "héllo")"#), Value::int(5));
    assert_eq!(eval(r#"(string-length "λ")"#), Value::int(1));
    assert_eq!(eval(r#"(string-length "日本語")"#), Value::int(3));
    assert_eq!(eval(r#"(string-length "😀")"#), Value::int(1));
    assert_eq!(eval(r#"(string-length "")"#), Value::int(0));
}

#[test]
fn test_unicode_substring() {
    // substring should use character indices, not byte indices
    assert_eq!(eval(r#"(substring "héllo" 0 1)"#), Value::string("h"));
    assert_eq!(eval(r#"(substring "héllo" 1 2)"#), Value::string("é"));
    assert_eq!(eval(r#"(substring "héllo" 0 5)"#), Value::string("héllo"));
    assert_eq!(eval(r#"(substring "日本語" 1 3)"#), Value::string("本語"));
    assert_eq!(eval(r#"(substring "😀🎉" 0 1)"#), Value::string("😀"));
    assert_eq!(eval(r#"(substring "😀🎉" 1 2)"#), Value::string("🎉"));
}

#[test]
fn test_unicode_string_ref() {
    assert_eq!(eval(r#"(string-ref "héllo" 1)"#), Value::char('é'));
    assert_eq!(eval(r#"(string-ref "日本語" 2)"#), Value::char('語'));
}

#[test]
fn test_unicode_string_pad() {
    // Padding should count characters, not bytes
    assert_eq!(eval(r#"(string/pad-left "éx" 5)"#), Value::string("   éx"));
    assert_eq!(eval(r#"(string/pad-right "éx" 5)"#), Value::string("éx   "));
    // Already at or past width
    assert_eq!(
        eval(r#"(string/pad-left "héllo" 3)"#),
        Value::string("héllo")
    );
}

#[test]
fn test_unicode_length_consistency() {
    // length and string-length should agree on character count
    assert_eq!(eval(r#"(length "héllo")"#), Value::int(5));
    assert_eq!(eval(r#"(count "héllo")"#), Value::int(5));
}

#[test]
fn test_dissoc_hashmap() {
    // dissoc should work on hashmaps and preserve type
    assert_eq!(
        eval("(hashmap/contains? (dissoc (hashmap/new :a 1 :b 2 :c 3) :b) :b)"),
        Value::bool(false)
    );
    assert_eq!(
        eval("(count (dissoc (hashmap/new :a 1 :b 2 :c 3) :b))"),
        Value::int(2)
    );
}

#[test]
fn test_merge_hashmap() {
    // merge should work with hashmaps
    assert_eq!(
        eval("(count (merge (hashmap/new :a 1) (hashmap/new :b 2)))"),
        Value::int(2)
    );
    // merge mixed: Map + HashMap
    assert_eq!(
        eval("(count (merge {:a 1} (hashmap/new :b 2)))"),
        Value::int(2)
    );
}

#[test]
fn test_map_entries_hashmap() {
    // map/entries should work on hashmaps
    assert_eq!(
        eval("(length (map/entries (hashmap/new :a 1 :b 2)))"),
        Value::int(2)
    );
}

#[test]
fn test_map_ops_hashmap() {
    // map/map-vals on hashmap
    assert_eq!(
        eval(
            "(get (hashmap/to-map (map/map-vals (lambda (v) (* v 2)) (hashmap/new :a 1 :b 2))) :a)"
        ),
        Value::int(2)
    );
    // map/filter on hashmap
    assert_eq!(
        eval("(count (map/filter (lambda (k v) (> v 1)) (hashmap/new :a 1 :b 2 :c 3)))"),
        Value::int(2)
    );
    // map/select-keys on hashmap
    assert_eq!(
        eval("(count (map/select-keys (hashmap/new :a 1 :b 2 :c 3) (list :a :c)))"),
        Value::int(2)
    );
    // map/map-keys on hashmap
    assert_eq!(
        eval("(count (map/map-keys (lambda (k) k) (hashmap/new :a 1 :b 2)))"),
        Value::int(2)
    );
    // map/update on hashmap
    assert_eq!(
        eval("(hashmap/get (map/update (hashmap/new :a 1) :a (lambda (v) (+ v 10))) :a)"),
        Value::int(11)
    );
}

#[test]
fn test_string_byte_length() {
    // ASCII: each char is 1 byte
    assert_eq!(eval(r#"(string/byte-length "hello")"#), Value::int(5));
    // UTF-8 multi-byte: "é" is 2 bytes, so "héllo" is 6 bytes
    assert_eq!(eval(r#"(string/byte-length "héllo")"#), Value::int(6));
    // CJK: each char is 3 bytes
    assert_eq!(eval(r#"(string/byte-length "日本語")"#), Value::int(9));
    // Empty string
    assert_eq!(eval(r#"(string/byte-length "")"#), Value::int(0));
}

#[test]
fn test_string_codepoints() {
    assert_eq!(eval_to_string(r#"(string/codepoints "ABC")"#), "(65 66 67)");
    // "é" is U+00E9 = 233
    assert_eq!(eval_to_string(r#"(string/codepoints "é")"#), "(233)");
    assert_eq!(eval_to_string(r#"(string/codepoints "")"#), "()");
}

#[test]
fn test_string_from_codepoints() {
    assert_eq!(
        eval(r#"(string/from-codepoints (list 65 66 67))"#),
        Value::string("ABC")
    );
    assert_eq!(
        eval(r#"(string/from-codepoints (list 233))"#),
        Value::string("é")
    );
    assert_eq!(
        eval(r#"(string/from-codepoints (list))"#),
        Value::string("")
    );
}

#[test]
fn test_string_from_codepoints_roundtrip() {
    assert_eq!(
        eval(r#"(string/from-codepoints (string/codepoints "Hello 世界"))"#),
        Value::string("Hello 世界")
    );
}

#[test]
fn test_string_normalize() {
    // NFC normalization: combining e + acute accent → é
    // U+0065 (e) + U+0301 (combining acute) → U+00E9 (é) in NFC
    assert_eq!(
        eval(r#"(string/normalize "e\u0301" :nfc)"#),
        Value::string("é")
    );
    // NFD decomposition: é → e + combining acute
    assert_eq!(
        eval(r#"(string-length (string/normalize "é" :nfd))"#),
        Value::int(2)
    );
    // NFKC: ﬁ ligature → "fi"
    assert_eq!(
        eval(r#"(string/normalize "\uFB01" :nfkc)"#),
        Value::string("fi")
    );
    // NFKD: ﬁ ligature → "fi"
    assert_eq!(
        eval(r#"(string/normalize "\uFB01" :nfkd)"#),
        Value::string("fi")
    );
    // String form names also work
    assert_eq!(
        eval(r#"(string/normalize "e\u0301" "NFC")"#),
        Value::string("é")
    );
}

#[test]
fn test_string_foldcase() {
    assert_eq!(eval(r#"(string/foldcase "HELLO")"#), Value::string("hello"));
    assert_eq!(
        eval(r#"(string/foldcase "Hello World")"#),
        Value::string("hello world")
    );
    // True Unicode case folding: German sharp s folds to "ss"
    // (this is the whole point of foldcase vs string/lower, which leaves "ß")
    assert_eq!(
        eval(r#"(string/foldcase "Straße")"#),
        Value::string("strasse")
    );
    // Final sigma folds the same as medial sigma, so caseless comparison works
    assert_eq!(eval(r#"(string/foldcase "ΩΣ")"#), Value::string("ωσ"));
    assert_eq!(
        eval(r#"(string/foldcase "ὈΔΥΣΣΕΎΣ")"#),
        eval(r#"(string/foldcase "ὀδυσσεύς")"#)
    );
    // string/lower still leaves the sharp s untouched (the two diverge)
    assert_eq!(eval(r#"(string/lower "Straße")"#), Value::string("straße"));
}

#[test]
fn test_string_ci_equal_folding() {
    // case-insensitive equality must use full folding, not plain lowercase,
    // so "ß" and "SS" compare equal
    assert_eq!(
        eval(r#"(string-ci=? "Straße" "STRASSE")"#),
        Value::bool(true)
    );
    assert_eq!(
        eval(r#"(string-ci=? "ΣΙΣΥΦΟΣ" "σισυφος")"#),
        Value::bool(true)
    );
}

// ─── Audit regressions (stdlib semantic-divergence sweep) ───────────────────

#[test]
fn test_typed_array_make_negative_length_errors() {
    // A negative length must error gracefully, not abort the process with a
    // Rust capacity-overflow panic (length wraps through `as usize`).
    assert!(eval_err("(f64-array/make -1)")
        .to_string()
        .contains("non-negative"));
    assert!(eval_err("(i64-array/make -5)")
        .to_string()
        .contains("non-negative"));
    // valid lengths still work
    assert_eq!(eval("(f64-array/length (f64-array/make 3))"), Value::int(3));
}

#[test]
fn test_int_rejects_non_finite_and_out_of_range() {
    // `int` truncates toward zero but must reject NaN/inf/out-of-range like its
    // sibling `truncate`, not saturate to i64::MIN/MAX or 0.
    assert!(eval_err("(int (/ 1.0 0.0))").to_string().contains("int"));
    assert!(eval_err("(int math/nan)").to_string().contains("int"));
    // 1.0e19 is beyond i64::MAX (~9.2e18); relies on scientific-notation literals (LEX-1)
    assert!(eval_err("(int 1.0e19)").to_string().contains("int"));
    // ordinary truncation unchanged
    assert_eq!(eval("(int 3.9)"), Value::int(3));
    assert_eq!(eval("(int -3.9)"), Value::int(-3));
}

#[test]
fn test_math_clamp_propagates_nan() {
    // NaN must propagate, not silently become the low bound (f64::max drops NaN).
    let v = eval("(math/clamp math/nan 0.0 10.0)");
    assert!(
        v.as_float().is_some_and(|f| f.is_nan()),
        "expected NaN, got {v}"
    );
    // ordinary clamping unchanged
    assert_eq!(eval("(math/clamp 15.0 0.0 10.0)"), Value::float(10.0));
    assert_eq!(eval("(math/clamp -5.0 0.0 10.0)"), Value::float(0.0));
}

#[test]
fn test_exit_rejects_non_numeric_status() {
    // A non-numeric status must error rather than silently exit(0), which would
    // turn an intended failure into success. (We only exercise the error path —
    // a valid (exit n) would terminate the test process.)
    assert!(eval_err(r#"(exit "x")"#)
        .to_string()
        .to_lowercase()
        .contains("type"));
}

#[test]
fn test_negative_fractional_timestamp_floors() {
    // -0.5s is 1969-12-31 23:59:59.5 UTC; decomposition must floor, not
    // truncate toward zero (which wrongly yields 1970-01-01).
    assert_eq!(
        eval(r#"(time/format -0.5 "%Y-%m-%d %H:%M:%S")"#),
        Value::string("1969-12-31 23:59:59")
    );
    assert_eq!(eval("(get (time/date-parts -0.5) :year)"), Value::int(1969));
    assert_eq!(eval("(get (time/date-parts -0.5) :month)"), Value::int(12));
    assert_eq!(eval("(get (time/date-parts -0.5) :day)"), Value::int(31));
}

#[test]
fn test_string_ci_equal() {
    assert_eq!(eval(r#"(string-ci=? "Hello" "hello")"#), Value::bool(true));
    assert_eq!(eval(r#"(string-ci=? "ABC" "abc")"#), Value::bool(true));
    assert_eq!(eval(r#"(string-ci=? "hello" "world")"#), Value::bool(false));
    assert_eq!(eval(r#"(string-ci=? "" "")"#), Value::bool(true));
}

#[test]
fn test_llm_pricing_status() {
    let interp = Interpreter::new();
    let result = interp.eval_str("(llm/pricing-status)").unwrap();
    if let Some(m) = result.as_map_rc() {
        assert!(m.contains_key(&Value::keyword("source")));
    } else {
        panic!("expected map, got {result}");
    }
}

#[test]
fn test_budget_with_unknown_model_does_not_error() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str("(begin (llm/set-budget 1.00) (llm/budget-remaining))")
        .unwrap();
    if let Some(m) = result.as_map_rc() {
        assert!(m.contains_key(&Value::keyword("limit")));
    } else {
        panic!("expected map, got {result}");
    }
}

// --- Lisp-defined providers ---

#[test]
fn test_define_provider_registers_and_sets_default() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :mock
            {:complete (fn (req) "hello from mock")
             :default-model "mock-v1"})
          (llm/current-provider))"#,
        )
        .unwrap();
    if let Some(m) = result.as_map_rc() {
        assert_eq!(
            m.get(&Value::keyword("name")),
            Some(&Value::keyword("mock"))
        );
        assert_eq!(
            m.get(&Value::keyword("model")),
            Some(&Value::string("mock-v1"))
        );
    } else {
        panic!("expected map, got {result}");
    }
}

#[test]
fn test_define_provider_appears_in_list() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :test-prov
            {:complete (fn (req) "ok")
             :default-model "t1"})
          (> (length (llm/list-providers)) 0))"#,
        )
        .unwrap();
    assert_eq!(result, Value::bool(true));
}

#[test]
fn test_define_provider_complete_returns_string() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :echo
            {:complete (fn (req)
              (string-append "echo: " (:content (first (:messages req)))))
             :default-model "echo-1"})
          (llm/complete "hello world"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("echo: hello world"));
}

#[test]
fn test_define_provider_complete_returns_map() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :map-prov
            {:complete (fn (req)
              {:content "map response"
               :role "assistant"
               :usage {:prompt-tokens 10 :completion-tokens 5}})
             :default-model "map-1"})
          (llm/complete "test"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("map response"));
}

#[test]
fn test_define_provider_receives_model() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :model-check
            {:complete (fn (req) (:model req))
             :default-model "default-model"})
          (llm/complete "test" {:model "custom-model"}))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("custom-model"));
}

#[test]
fn test_define_provider_receives_system_prompt() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :sys-check
            {:complete (fn (req)
              (if (nil? (:system req)) "no system" (:system req)))
             :default-model "s1"})
          (llm/complete "test" {:system "be helpful"}))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("be helpful"));
}

#[test]
fn test_define_provider_uses_default_model() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :defmodel
            {:complete (fn (req) (:model req))
             :default-model "my-default"})
          (llm/complete "test"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("my-default"));
}

#[test]
fn test_define_provider_requires_complete() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(llm/define-provider :bad {:default-model "x"})"#);
    let err = result
        .expect_err("missing :complete must error")
        .to_string();
    assert!(
        err.contains("complete"),
        "error should mention the missing :complete field, got: {err}"
    );
}

#[test]
fn test_define_provider_validates_complete_is_function() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(r#"(llm/define-provider :bad {:complete "not a function" :default-model "x"})"#);
    let err = result
        .expect_err(":complete must be a function or this must error")
        .to_string();
    let lowered = err.to_lowercase();
    assert!(
        (lowered.contains("complete") || lowered.contains("string"))
            && (lowered.contains("function")
                || lowered.contains("callable")
                || lowered.contains("lambda")),
        "error should explain that :complete must be a function, got: {err}"
    );
}

#[test]
fn test_define_provider_with_closure() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (define prefix "PREFIX")
          (llm/define-provider :closure-prov
            {:complete (fn (req)
              (string-append prefix ": " (:content (first (:messages req)))))
             :default-model "c1"})
          (llm/complete "hi"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("PREFIX: hi"));
}

#[test]
fn test_define_provider_receives_max_tokens() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :tok-check
            {:complete (fn (req)
              (number->string (:max-tokens req)))
             :default-model "t1"})
          (llm/complete "test" {:max-tokens 42}))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("42"));
}

#[test]
fn test_define_provider_switch_back() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :prov-a
            {:complete (fn (req) "from A") :default-model "a1"})
          (llm/define-provider :prov-b
            {:complete (fn (req) "from B") :default-model "b1"})
          (llm/set-default :prov-a)
          (llm/complete "test"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("from A"));
}

// --- OpenAI-compatible fallback ---

#[test]
fn test_configure_unknown_provider_without_base_url_errors() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(llm/configure :unknown-provider {:api-key "test"})"#);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("base-url"),
        "error should mention base-url: {err}"
    );
}

#[test]
fn test_configure_unknown_provider_without_api_key_errors() {
    let interp = Interpreter::new();
    let result =
        interp.eval_str(r#"(llm/configure :unknown-provider {:base-url "http://example.com/v1"})"#);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("api-key"),
        "error should mention api-key: {err}"
    );
}

#[test]
fn test_define_provider_error_propagation() {
    let interp = Interpreter::new();
    let result = interp.eval_str(
        r#"(begin
          (llm/define-provider :err-prov
            {:complete (fn (req) (error "provider failed"))
             :default-model "e1"})
          (try (llm/complete "test") (catch e "caught")))"#,
    );
    // Should catch the error, not panic
    assert_eq!(result.unwrap(), Value::string("caught"));
}

#[test]
fn test_define_provider_nil_return_errors() {
    let interp = Interpreter::new();
    let result = interp.eval_str(
        r#"(begin
          (llm/define-provider :nil-prov
            {:complete (fn (req) nil)
             :default-model "n1"})
          (llm/complete "test"))"#,
    );
    assert!(result.is_err());
}

#[test]
fn test_define_provider_redefine_uses_latest() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :redef
            {:complete (fn (req) "first") :default-model "r1"})
          (llm/define-provider :redef
            {:complete (fn (req) "second") :default-model "r2"})
          (llm/complete "test"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("second"));
}

#[test]
fn test_define_provider_temperature_passthrough() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :temp-check
            {:complete (fn (req)
              (number->string (:temperature req)))
             :default-model "t1"})
          (llm/complete "test" {:temperature 0.7}))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("0.7"));
}

#[test]
fn test_define_provider_default_model_fallback() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :nomodel
            {:complete (fn (req) (:model req))})
          (llm/complete "test"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("default"));
}

#[test]
fn test_define_provider_integer_return_errors() {
    let interp = Interpreter::new();
    let result = interp.eval_str(
        r#"(begin
          (llm/define-provider :int-prov
            {:complete (fn (req) 42)
             :default-model "i1"})
          (llm/complete "test"))"#,
    );
    assert!(result.is_err());
}

#[test]
fn test_define_provider_tool_calls_response() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :tool-prov
            {:complete (fn (req)
              {:content "I'll use a tool"
               :stop-reason "tool_use"
               :tool-calls [{:id "tc_1" :name "read-file" :arguments {:path "test.txt"}}]})
             :default-model "t1"})
          (llm/complete "test"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("I'll use a tool"));
}

#[test]
fn test_define_provider_stream_fallback() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
          (llm/define-provider :stream-prov
            {:complete (fn (req) "streamed response")
             :default-model "s1"})
          (llm/stream "test"))"#,
        )
        .unwrap();
    assert_eq!(result, Value::string("streamed response"));
}

#[test]
fn test_sandbox_shell_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::SHELL);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(shell "echo hi")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_shell_allowed_when_other_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::NETWORK);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(shell "echo hi")"#);
    assert!(
        result.is_ok(),
        "shell should be allowed when only network is denied"
    );
}

#[test]
fn test_sandbox_fs_write_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(file/write "/tmp/sema-sandbox-test.txt" "hi")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_fs_read_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(file/exists? "/tmp")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

// EVAL-3: `import` must be gated behind FS_READ like other filesystem reads.
// Without the sandbox check, a restricted interpreter could read arbitrary
// source files off disk via (import ...).
#[test]
fn test_sandbox_import_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(import "/etc/hosts")"#);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Permission denied"));
}

#[test]
fn test_sandbox_env_read_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ENV_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(env "HOME")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_env_read_denies_implicit_env_readers() {
    // sys/home-dir, sys/user, sys/cwd, sys/temp-dir all read process env state
    // (HOME/USER/PWD/TMPDIR) and must be gated by ENV_READ for consistency with
    // (env "...") and (sys/env-all).
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ENV_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    for expr in [
        "(sys/home-dir)",
        "(sys/user)",
        "(sys/cwd)",
        "(sys/temp-dir)",
    ] {
        let result = interp.eval_str(expr);
        assert!(result.is_err(), "{expr} should be denied under ENV_READ");
        assert_permission_denied(&result.unwrap_err());
    }
}

#[test]
fn test_sandbox_shell_denied_by_process_only() {
    // shell launches a child process, so denying PROCESS must block it even when
    // SHELL is allowed.
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::PROCESS);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(shell "echo hi")"#);
    assert!(
        result.is_err(),
        "shell should be denied when PROCESS is denied"
    );
    let err = result.unwrap_err();
    assert_permission_denied(&err);
    assert!(
        err.to_string().contains("shell"),
        "error should mention shell function: {err}"
    );
}

#[test]
fn test_sandbox_env_write_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ENV_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(sys/set-env "SEMA_TEST_SANDBOX" "val")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_process_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::PROCESS);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(sys/pid)"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_network_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::NETWORK);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(http/get "https://example.com")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_safe_functions_always_work() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ALL);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    assert_eq!(interp.eval_str("(+ 1 2)").unwrap(), Value::int(3));
    assert_eq!(
        interp.eval_str(r#"(string-append "a" "b")"#).unwrap(),
        Value::string("ab")
    );
}

#[test]
fn test_sandbox_println_always_works() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ALL);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(println "hello")"#);
    assert!(result.is_ok(), "println should never be sandboxed");
}

#[test]
fn test_sandbox_strict_preset() {
    let sandbox = sema_core::Sandbox::parse_cli("strict").unwrap();
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(shell "echo hi")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_parse_cli_multiple() {
    let sandbox = sema_core::Sandbox::parse_cli("no-shell,no-network").unwrap();
    let interp = Interpreter::new_with_sandbox(&sandbox);
    // shell denied
    assert!(interp.eval_str(r#"(shell "echo hi")"#).is_err());
    // fs-read still allowed
    assert!(interp.eval_str(r#"(file/exists? "/tmp")"#).is_ok());
}

#[test]
#[cfg(unix)]
fn test_sandbox_unrestricted_by_default() {
    let sandbox = sema_core::Sandbox::allow_all();
    let interp = Interpreter::new_with_sandbox(&sandbox);
    // Everything should work (runs a real `echo` and reads HOME — unix-specific)
    assert!(interp.eval_str(r#"(shell "echo hi")"#).is_ok());
    assert!(interp.eval_str(r#"(file/exists? "/tmp")"#).is_ok());
    assert!(interp.eval_str(r#"(env "HOME")"#).is_ok());
}

// === Sandbox: comprehensive fs-read gating ===

#[test]
fn test_sandbox_fs_read_all_functions_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);

    let fs_read_fns = [
        r#"(file/read "/tmp/test.txt")"#,
        r#"(file/exists? "/tmp")"#,
        r#"(file/is-directory? "/tmp")"#,
        r#"(file/is-file? "/tmp/test.txt")"#,
        r#"(file/is-symlink? "/tmp/test.txt")"#,
        r#"(file/list "/tmp")"#,
        r#"(file/read-lines "/tmp/test.txt")"#,
        r#"(file/info "/tmp")"#,
        r#"(path/absolute ".")"#,
    ];

    for expr in &fs_read_fns {
        let result = interp.eval_str(expr);
        assert!(
            result.is_err(),
            "Expected {expr} to be denied, but it succeeded"
        );
        assert_permission_denied(&result.unwrap_err());
    }
}

// === Sandbox: comprehensive fs-write gating ===

#[test]
fn test_sandbox_fs_write_all_functions_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);

    let fs_write_fns = [
        r#"(file/write "/tmp/sema-sandbox-test.txt" "hi")"#,
        r#"(file/append "/tmp/sema-sandbox-test.txt" "more")"#,
        r#"(file/delete "/tmp/sema-sandbox-test.txt")"#,
        r#"(file/rename "/tmp/sema-sandbox-a.txt" "/tmp/sema-sandbox-b.txt")"#,
        r#"(file/mkdir "/tmp/sema-sandbox-dir")"#,
        r#"(file/write-lines "/tmp/sema-sandbox-test.txt" '("a" "b"))"#,
        r#"(file/copy "/tmp/sema-sandbox-a.txt" "/tmp/sema-sandbox-b.txt")"#,
    ];

    for expr in &fs_write_fns {
        let result = interp.eval_str(expr);
        assert!(
            result.is_err(),
            "Expected {expr} to be denied, but it succeeded"
        );
        assert_permission_denied(&result.unwrap_err());
    }
}

#[test]
fn test_sandbox_fs_write_allows_read() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    // fs-read should still work when only fs-write is denied
    assert!(interp.eval_str(r#"(file/exists? "/tmp")"#).is_ok());
    assert!(interp.eval_str(r#"(file/is-directory? "/tmp")"#).is_ok());
}

// === Sandbox: comprehensive system/process gating ===

#[test]
fn test_sandbox_process_all_functions_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::PROCESS);
    let interp = Interpreter::new_with_sandbox(&sandbox);

    let process_fns = [
        "(exit 0)", // would exit, but sandbox catches it first
        "(sys/pid)",
        "(sys/args)",
        r#"(sys/which "ls")"#,
    ];

    for expr in &process_fns {
        let result = interp.eval_str(expr);
        assert!(
            result.is_err(),
            "Expected {expr} to be denied, but it succeeded"
        );
        assert_permission_denied(&result.unwrap_err());
    }
}

#[test]
fn test_sandbox_process_allows_safe_sys_functions() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::PROCESS);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    // These sys functions should NOT be gated by PROCESS
    assert!(interp.eval_str("(sys/platform)").is_ok());
    assert!(interp.eval_str("(sys/arch)").is_ok());
    assert!(interp.eval_str("(sys/os)").is_ok());
    assert!(interp.eval_str("(sys/cwd)").is_ok());
    assert!(interp.eval_str("(time-ms)").is_ok());
}

// === Sandbox: comprehensive env gating ===

#[test]
fn test_sandbox_env_read_all_functions_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ENV_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);

    assert!(interp.eval_str(r#"(env "HOME")"#).is_err());
    assert!(interp.eval_str("(sys/env-all)").is_err());
}

#[test]
fn test_sandbox_env_write_allows_read() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ENV_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    // env read should still work
    assert!(interp.eval_str(r#"(env "HOME")"#).is_ok());
    // env write should be denied
    assert!(interp
        .eval_str(r#"(sys/set-env "SEMA_TEST_X" "v")"#)
        .is_err());
}

// === Sandbox: comprehensive network gating ===

#[test]
fn test_sandbox_network_all_functions_denied() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::NETWORK);
    let interp = Interpreter::new_with_sandbox(&sandbox);

    let net_fns = [
        r#"(http/get "https://example.com")"#,
        r#"(http/post "https://example.com" "body")"#,
        r#"(http/put "https://example.com" "body")"#,
        r#"(http/delete "https://example.com")"#,
        r#"(http/request "GET" "https://example.com")"#,
    ];

    for expr in &net_fns {
        let result = interp.eval_str(expr);
        assert!(
            result.is_err(),
            "Expected {expr} to be denied, but it succeeded"
        );
        assert_permission_denied(&result.unwrap_err());
    }
}

// === Sandbox: try/catch interaction ===

#[test]
fn test_sandbox_error_catchable_with_try() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::SHELL);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(
        r#"
        (try
          (shell "echo hi")
          (catch e "caught"))
    "#,
    );
    assert_eq!(result.unwrap(), Value::string("caught"));
}

#[test]
fn test_sandbox_error_message_accessible_in_catch() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::SHELL);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp
        .eval_str(
            r#"
        (try
          (shell "echo hi")
          (catch e e))
    "#,
        )
        .unwrap();
    let msg = result.to_string();
    assert!(
        msg.contains("Permission denied") || msg.contains("permission-denied"),
        "Error value should contain permission info: {msg}"
    );
}

// === Sandbox: combined capabilities ===

#[test]
fn test_sandbox_deny_multiple_caps_union() {
    let denied = sema_core::Caps::SHELL
        .union(sema_core::Caps::NETWORK)
        .union(sema_core::Caps::FS_WRITE);
    let sandbox = sema_core::Sandbox::deny(denied);
    let interp = Interpreter::new_with_sandbox(&sandbox);

    // all three denied
    assert!(interp.eval_str(r#"(shell "echo hi")"#).is_err());
    assert!(interp
        .eval_str(r#"(http/get "https://example.com")"#)
        .is_err());
    assert!(interp.eval_str(r#"(file/write "/tmp/x" "y")"#).is_err());

    // these should still work
    assert!(interp.eval_str(r#"(file/exists? "/tmp")"#).is_ok());
    assert!(interp.eval_str(r#"(env "HOME")"#).is_ok());
    assert!(interp.eval_str("(+ 1 2)").is_ok());
}

// === Sandbox: safe functions under maximum restriction ===

#[test]
fn test_sandbox_all_denied_safe_functions_comprehensive() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::ALL);
    let interp = Interpreter::new_with_sandbox(&sandbox);

    // arithmetic
    assert_eq!(interp.eval_str("(+ 1 2)").unwrap(), Value::int(3));
    // strings
    assert_eq!(
        interp.eval_str(r#"(string-length "hello")"#).unwrap(),
        Value::int(5)
    );
    // lists
    assert_eq!(interp.eval_str("(length '(1 2 3))").unwrap(), Value::int(3));
    // maps
    assert_eq!(
        interp.eval_str(r#"(get {:a 1} :a)"#).unwrap(),
        Value::int(1)
    );
    // predicates
    assert_eq!(interp.eval_str("(number? 42)").unwrap(), Value::bool(true));
    // display/print (I/O but ungated)
    assert!(interp.eval_str(r#"(println "test")"#).is_ok());
    assert!(interp.eval_str(r#"(display "test")"#).is_ok());
    assert!(interp.eval_str("(newline)").is_ok());
    // read/parse (not file I/O)
    assert!(interp.eval_str(r#"(read "(+ 1 2)")"#).is_ok());
    // path pure operations (no filesystem access)
    assert_eq!(
        interp
            .eval_str(r#"(path/join "a" "b" "c")"#)
            .unwrap()
            .as_str()
            .unwrap()
            .replace('\\', "/"),
        "a/b/c"
    );
    assert_eq!(
        interp
            .eval_str(r#"(path/basename "/foo/bar.txt")"#)
            .unwrap(),
        Value::string("bar.txt")
    );
    assert_eq!(
        interp.eval_str(r#"(path/dirname "/foo/bar.txt")"#).unwrap(),
        Value::string("/foo")
    );
    assert_eq!(
        interp
            .eval_str(r#"(path/extension "/foo/bar.txt")"#)
            .unwrap(),
        Value::string("txt")
    );
    // time (ungated)
    assert!(interp.eval_str("(time-ms)").is_ok());
    // sys info (ungated — pure constants)
    assert!(interp.eval_str("(sys/platform)").is_ok());
    assert!(interp.eval_str("(sys/arch)").is_ok());
    assert!(interp.eval_str("(sys/os)").is_ok());
    // sys/cwd is ENV_READ-gated (it leaks $PWD), so it must NOT be in the safe list.
    assert!(interp.eval_str("(sys/cwd)").is_err());
    // regex
    assert!(interp.eval_str(r#"(regex/match "\\d+" "abc123")"#).is_ok());
    // json
    assert!(interp.eval_str(r#"(json/decode "{\"a\":1}")"#).is_ok());
    // math
    assert!(interp.eval_str("(sqrt 4)").is_ok());
    // crypto
    assert!(interp.eval_str(r#"(hash/sha256 "hello")"#).is_ok());
    // error throwing (ungated)
    assert!(interp
        .eval_str(r#"(try (error "boom") (catch e "ok"))"#)
        .is_ok());
}

// === Task 8: message/with-image ===

#[test]
fn test_message_with_image_creates_message() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
                (define msg (message/with-image :user "Describe this" (bytevector 137 80 78 71)))
                (message? msg))"#,
        )
        .unwrap();
    assert_eq!(result, Value::bool(true));
}

#[test]
fn test_message_with_image_role() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
                (define msg (message/with-image :user "What is this?" (bytevector 1 2 3)))
                (message/role msg))"#,
        )
        .unwrap();
    assert_eq!(result.to_string(), ":user");
}

// === Task 4: file/glob ===

#[test]
fn test_file_glob() {
    let interp = Interpreter::new();
    // Use absolute path to workspace root for reliable globbing
    let workspace = env!("CARGO_MANIFEST_DIR").replace("/crates/sema", "");
    let expr = format!(r#"(length (file/glob "{workspace}/crates/*/Cargo.toml"))"#);
    let result = interp.eval_str(&expr).unwrap();
    let count = result.as_int().unwrap();
    assert!(
        count >= 7,
        "expected at least 7 crate Cargo.toml files, got {count}"
    );
}

#[test]
fn test_file_glob_no_matches() {
    assert_eq!(
        eval(r#"(file/glob "nonexistent-dir-xyz/*.nothing")"#).to_string(),
        "()"
    );
}

#[test]
fn test_file_glob_returns_list_of_strings() {
    let interp = Interpreter::new();
    let manifest = env!("CARGO_MANIFEST_DIR").replace('\\', "/");
    let result = interp
        .eval_str(&format!(
            r#"(string? (car (file/glob "{manifest}/Cargo.*")))"#
        ))
        .unwrap();
    assert_eq!(result, Value::bool(true));
}

// === Task 3: Path utilities ===

#[test]
fn test_path_ext() {
    assert_eq!(eval(r#"(path/ext "photo.jpg")"#), Value::string("jpg"));
    assert_eq!(eval(r#"(path/ext "archive.tar.gz")"#), Value::string("gz"));
    assert_eq!(eval(r#"(path/ext "Makefile")"#), Value::string(""));
    assert_eq!(
        eval(r#"(path/ext "/home/user/.bashrc")"#),
        Value::string("")
    );
}

#[test]
fn test_path_stem() {
    assert_eq!(eval(r#"(path/stem "photo.jpg")"#), Value::string("photo"));
    assert_eq!(
        eval(r#"(path/stem "/tmp/data.csv")"#),
        Value::string("data")
    );
    assert_eq!(eval(r#"(path/stem "Makefile")"#), Value::string("Makefile"));
}

#[test]
fn test_path_dir() {
    assert_eq!(eval(r#"(path/dir "/tmp/data.csv")"#), Value::string("/tmp"));
    assert_eq!(eval(r#"(path/dir "data.csv")"#), Value::string(""));
    assert_eq!(
        eval(r#"(path/dir "/home/user/.config/app.toml")"#),
        Value::string("/home/user/.config")
    );
}

#[test]
fn test_path_filename() {
    assert_eq!(
        eval(r#"(path/filename "/tmp/data.csv")"#),
        Value::string("data.csv")
    );
    assert_eq!(
        eval(r#"(path/filename "data.csv")"#),
        Value::string("data.csv")
    );
}

#[test]
fn test_path_join_multi() {
    assert_eq!(
        eval_path(r#"(path/join "/tmp" "data.csv")"#),
        "/tmp/data.csv"
    );
    assert_eq!(
        eval_path(r#"(path/join "/home" "user" ".config")"#),
        "/home/user/.config"
    );
}

#[test]
fn test_path_absolute_predicate() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(r#"(path/absolute? "/tmp/data.csv")"#)
        .unwrap();
    assert_eq!(result, Value::bool(true));
    let result = interp.eval_str(r#"(path/absolute? "data.csv")"#).unwrap();
    assert_eq!(result, Value::bool(false));
}

// === Task 2: base64/encode-bytes and base64/decode-bytes ===

#[test]
fn test_base64_encode_bytes() {
    assert_eq!(
        eval(r#"(base64/encode-bytes (bytevector 72 101 108 108 111))"#),
        Value::string("SGVsbG8=")
    );
}

#[test]
fn test_base64_decode_bytes() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(r#"(bytevector-length (base64/decode-bytes "SGVsbG8="))"#)
        .unwrap();
    assert_eq!(result, Value::int(5));
}

#[test]
fn test_base64_roundtrip_bytes() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
                (define bv (bytevector 0 1 255 128 64))
                (define encoded (base64/encode-bytes bv))
                (define decoded (base64/decode-bytes encoded))
                (= bv decoded))"#,
        )
        .unwrap();
    assert_eq!(result, Value::bool(true));
}

#[test]
fn test_base64_encode_bytes_type_error() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(base64/encode-bytes "not a bytevector")"#);
    assert!(result.is_err());
}

#[test]
fn test_base64_decode_bytes_invalid() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(base64/decode-bytes "!!!invalid!!!")"#);
    assert!(result.is_err());
}

#[test]
fn test_base64_encode_bytes_empty() {
    assert_eq!(
        eval(r#"(base64/encode-bytes (bytevector))"#),
        Value::string("")
    );
}

// === Sandbox: load is gated by fs-read ===

// === Task 1: file/read-bytes and file/write-bytes ===

#[test]
fn test_file_read_bytes() {
    let interp = Interpreter::new();
    let path = temp_path("sema-test-bytes.txt");
    let result = interp
        .eval_str(&format!(
            r#"(begin
                (file/write "{path}" "ABC")
                (define bv (file/read-bytes "{path}"))
                (list (bytevector-length bv)
                      (bytevector-u8-ref bv 0)
                      (bytevector-u8-ref bv 1)
                      (bytevector-u8-ref bv 2)))"#,
        ))
        .unwrap();
    assert_eq!(result.to_string(), "(3 65 66 67)");
}

#[test]
fn test_file_read_bytes_not_found() {
    let interp = Interpreter::new();
    let path = temp_path("sema-nonexistent-xyz.bin");
    let result = interp.eval_str(&format!(r#"(file/read-bytes "{path}")"#));
    assert!(result.is_err());
}

#[test]
fn test_file_write_bytes() {
    let interp = Interpreter::new();
    let path = temp_path("sema-test-write-bytes.bin");
    let result = interp
        .eval_str(&format!(
            r#"(begin
                (file/write-bytes "{path}" (bytevector 72 101 108 108 111))
                (file/read "{path}"))"#,
        ))
        .unwrap();
    assert_eq!(result, Value::string("Hello"));
}

#[test]
fn test_file_write_bytes_type_error() {
    let interp = Interpreter::new();
    let path = temp_path("sema-foo.bin");
    let result = interp.eval_str(&format!(
        r#"(file/write-bytes "{path}" "not a bytevector")"#
    ));
    assert!(result.is_err());
}

#[test]
fn test_sandbox_load_denied_by_fs_read() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(load "nonexistent.sema")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

// === New Collection Functions (Laravel-inspired) ===

#[test]
fn test_list_reject() {
    assert_eq!(
        eval_to_string("(list/reject (fn (x) (> x 3)) (list 1 2 3 4 5))"),
        "(1 2 3)"
    );
    assert_eq!(
        eval_to_string("(list/reject (fn (x) (even? x)) (list 1 2 3 4 5))"),
        "(1 3 5)"
    );
    assert_eq!(
        eval_to_string("(list/reject (fn (x) #t) (list 1 2 3))"),
        "()"
    );
    assert_eq!(
        eval_to_string("(list/reject (fn (x) #f) (list 1 2 3))"),
        "(1 2 3)"
    );
}

#[test]
fn test_list_pluck() {
    assert_eq!(
        eval_to_string(
            r#"(list/pluck :name (list (hash-map :name "Alice" :age 30) (hash-map :name "Bob" :age 25)))"#
        ),
        r#"("Alice" "Bob")"#
    );
    assert_eq!(
        eval_to_string(r#"(list/pluck :missing (list (hash-map :a 1)))"#),
        "(nil)"
    );
    assert_eq!(eval_to_string("(list/pluck :x (list))"), "()");
}

#[test]
fn test_list_avg() {
    assert_eq!(eval("(list/avg (list 2 4 6))"), Value::float(4.0));
    assert_eq!(eval("(list/avg (list 1 2 3 4))"), Value::float(2.5));
    assert_eq!(eval("(list/avg (list 10))"), Value::float(10.0));
    assert_eq!(eval("(list/avg (list 1.5 2.5))"), Value::float(2.0));
}

#[test]
fn test_list_avg_empty_error() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(list/avg (list))").is_err());
}

#[test]
fn test_list_median() {
    // Odd count
    assert_eq!(eval("(list/median (list 3 1 2))"), Value::float(2.0));
    // Even count
    assert_eq!(eval("(list/median (list 1 2 3 4))"), Value::float(2.5));
    // Single element
    assert_eq!(eval("(list/median (list 5))"), Value::float(5.0));
    // Already sorted
    assert_eq!(eval("(list/median (list 1 2 3 4 5))"), Value::float(3.0));
}

#[test]
fn test_list_median_empty_error() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(list/median (list))").is_err());
}

#[test]
fn test_list_mode() {
    // Single mode
    assert_eq!(eval("(list/mode (list 1 2 2 3 3 3))"), Value::int(3));
    // Multiple modes returns a list
    assert_eq!(eval_to_string("(list/mode (list 1 1 2 2 3))"), "(1 2)");
    // All same
    assert_eq!(eval("(list/mode (list 5 5 5))"), Value::int(5));
}

#[test]
fn test_list_diff() {
    assert_eq!(
        eval_to_string("(list/diff (list 1 2 3 4 5) (list 3 4))"),
        "(1 2 5)"
    );
    assert_eq!(
        eval_to_string("(list/diff (list 1 2 3) (list 4 5 6))"),
        "(1 2 3)"
    );
    assert_eq!(
        eval_to_string("(list/diff (list 1 2 3) (list 1 2 3))"),
        "()"
    );
}

#[test]
fn test_list_intersect() {
    assert_eq!(
        eval_to_string("(list/intersect (list 1 2 3 4 5) (list 3 4 6))"),
        "(3 4)"
    );
    assert_eq!(
        eval_to_string("(list/intersect (list 1 2 3) (list 4 5 6))"),
        "()"
    );
}

#[test]
fn test_list_sliding() {
    assert_eq!(
        eval_to_string("(list/sliding (list 1 2 3 4 5) 2)"),
        "((1 2) (2 3) (3 4) (4 5))"
    );
    assert_eq!(
        eval_to_string("(list/sliding (list 1 2 3 4 5) 3)"),
        "((1 2 3) (2 3 4) (3 4 5))"
    );
    // With step
    assert_eq!(
        eval_to_string("(list/sliding (list 1 2 3 4 5 6) 2 3)"),
        "((1 2) (4 5))"
    );
    // Window larger than list
    assert_eq!(eval_to_string("(list/sliding (list 1 2) 5)"), "()");
}

#[test]
fn test_list_key_by() {
    assert_eq!(
        eval(
            r#"(begin
            (define people (list (hash-map :id 1 :name "Alice") (hash-map :id 2 :name "Bob")))
            (define keyed (list/key-by (fn (p) (get p :id)) people))
            (get (get keyed 2) :name))"#
        ),
        Value::string("Bob")
    );
}

#[test]
fn test_list_times() {
    assert_eq!(
        eval_to_string("(list/times 5 (fn (i) (* i i)))"),
        "(0 1 4 9 16)"
    );
    assert_eq!(eval_to_string("(list/times 3 (fn (i) (+ i 1)))"), "(1 2 3)");
    assert_eq!(eval_to_string("(list/times 0 (fn (i) i))"), "()");
}

#[test]
fn test_list_duplicates() {
    assert_eq!(
        eval_to_string("(list/duplicates (list 1 2 2 3 3 3 4))"),
        "(2 3)"
    );
    assert_eq!(eval_to_string("(list/duplicates (list 1 2 3))"), "()");
    assert_eq!(eval_to_string("(list/duplicates (list 1 1 1))"), "(1)");
}

#[test]
fn test_list_cross_join() {
    assert_eq!(
        eval_to_string("(list/cross-join (list 1 2) (list 3 4))"),
        "((1 3) (1 4) (2 3) (2 4))"
    );
    assert_eq!(
        eval_to_string(r#"(list/cross-join (list "a" "b") (list 1 2))"#),
        r#"(("a" 1) ("a" 2) ("b" 1) ("b" 2))"#
    );
    assert_eq!(eval_to_string("(list/cross-join (list) (list 1 2))"), "()");
}

#[test]
fn test_list_page() {
    assert_eq!(eval_to_string("(list/page (range 20) 1 5)"), "(0 1 2 3 4)");
    assert_eq!(eval_to_string("(list/page (range 20) 2 5)"), "(5 6 7 8 9)");
    assert_eq!(
        eval_to_string("(list/page (range 20) 4 5)"),
        "(15 16 17 18 19)"
    );
    // Beyond last page
    assert_eq!(eval_to_string("(list/page (range 20) 5 5)"), "()");
    // Partial last page
    assert_eq!(eval_to_string("(list/page (range 7) 2 5)"), "(5 6)");
}

#[test]
fn test_list_find() {
    assert_eq!(
        eval("(list/find (fn (x) (> x 3)) (list 1 2 3 4 5))"),
        Value::int(4)
    );
    assert_eq!(
        eval("(list/find (fn (x) (> x 10)) (list 1 2 3))"),
        Value::nil()
    );
    assert_eq!(eval("(list/find even? (list 1 3 4 5 6))"), Value::int(4));
}

#[test]
fn test_list_pad() {
    assert_eq!(eval_to_string("(list/pad (list 1 2 3) 5 0)"), "(1 2 3 0 0)");
    // Already long enough
    assert_eq!(eval_to_string("(list/pad (list 1 2 3) 2 0)"), "(1 2 3)");
    assert_eq!(eval_to_string("(list/pad (list) 3 nil)"), "(nil nil nil)");
}

#[test]
fn test_list_sole() {
    assert_eq!(
        eval("(list/sole (fn (x) (> x 3)) (list 1 2 3 4))"),
        Value::int(4)
    );
}

#[test]
fn test_list_sole_multiple_error() {
    let interp = Interpreter::new();
    let err = interp
        .eval_str("(list/sole (fn (x) (> x 2)) (list 1 2 3 4))")
        .expect_err("more than one match must error")
        .to_string()
        .to_lowercase();
    assert!(
        err.contains("more than one") || err.contains("multiple"),
        "error should distinguish multi-match case, got: {err}"
    );
}

#[test]
fn test_list_sole_none_error() {
    let interp = Interpreter::new();
    let err = interp
        .eval_str("(list/sole (fn (x) (> x 10)) (list 1 2 3))")
        .expect_err("no match must error")
        .to_string()
        .to_lowercase();
    assert!(
        err.contains("no match") || err.contains("no matching") || err.contains("none"),
        "error should distinguish no-match case, got: {err}"
    );
}

#[test]
fn test_list_join() {
    assert_eq!(
        eval(r#"(list/join (list "a" "b" "c") ", ")"#),
        Value::string(r#""a", "b", "c""#)
    );
    assert_eq!(
        eval(r#"(list/join (list "a" "b" "c") ", " " and ")"#),
        Value::string(r#""a", "b" and "c""#)
    );
    assert_eq!(
        eval(r#"(list/join (list "solo") ", ")"#),
        Value::string(r#""solo""#)
    );
    assert_eq!(eval(r#"(list/join (list) ", ")"#), Value::string(""));
    // With numbers
    assert_eq!(
        eval(r#"(list/join (list 1 2 3) ", " " and ")"#),
        Value::string("1, 2 and 3")
    );
}

#[test]
fn test_tap() {
    // tap should return the original value
    assert_eq!(eval("(tap 42 (fn (x) (+ x 1)))"), Value::int(42));
    assert_eq!(eval_to_string("(tap (list 1 2 3) (fn (x) nil))"), "(1 2 3)");
}

#[test]
fn test_map_sort_keys() {
    // Note: output order relies on BTreeMap sorted iteration
    // BTreeMap is already sorted, so this is a no-op for regular maps
    assert_eq!(
        eval_to_string("(map/entries (map/sort-keys (hash-map :b 2 :a 1 :c 3)))"),
        "((:a 1) (:b 2) (:c 3))"
    );
    // HashMap -> sorted map
    assert_eq!(
        eval_to_string(
            "(begin (define hm (hashmap/new :b 2 :a 1 :c 3)) (map/entries (map/sort-keys hm)))"
        ),
        "((:a 1) (:b 2) (:c 3))"
    );
}

#[test]
fn test_map_except() {
    // Note: output order relies on BTreeMap sorted iteration
    assert_eq!(
        eval_to_string("(map/entries (map/except (hash-map :a 1 :b 2 :c 3) (list :b)))"),
        "((:a 1) (:c 3))"
    );
    assert_eq!(
        eval_to_string("(map/entries (map/except (hash-map :a 1 :b 2 :c 3) (list :a :c)))"),
        "((:b 2))"
    );
    // Remove non-existing key
    assert_eq!(
        eval("(count (map/except (hash-map :a 1 :b 2) (list :z)))"),
        Value::int(2)
    );
}

#[test]
fn test_llm_cache_clear() {
    let result = eval("(llm/cache-clear)");
    assert_eq!(result, Value::int(0));
}

#[test]
fn test_llm_cache_stats_empty() {
    let result = eval("(llm/cache-stats)");
    let map = result.as_map_rc().expect("should be a map");
    assert!(map.contains_key(&Value::keyword("hits")));
    assert!(map.contains_key(&Value::keyword("misses")));
    assert!(map.contains_key(&Value::keyword("size")));
}

#[test]
fn test_llm_cache_key_generation() {
    let k1 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.5})"#);
    let k2 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.5})"#);
    assert_eq!(k1, k2);
    let k3 = eval(r#"(llm/cache-key "world" {:model "gpt-4" :temperature 0.5})"#);
    assert_ne!(k1, k3);
}

#[test]
fn test_llm_cache_key_different_model() {
    let k1 = eval(r#"(llm/cache-key "hello" {:model "gpt-4"})"#);
    let k2 = eval(r#"(llm/cache-key "hello" {:model "claude-3"})"#);
    assert_ne!(k1, k2);
}

#[test]
fn test_llm_cache_key_different_temperature() {
    let k1 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.0})"#);
    let k2 = eval(r#"(llm/cache-key "hello" {:model "gpt-4" :temperature 0.7})"#);
    assert_ne!(k1, k2);
}

#[test]
fn test_llm_providers_list() {
    let result = eval("(llm/providers)");
    assert!(result.as_list().is_some());
}

#[test]
fn test_llm_default_provider_none() {
    let result = eval("(llm/default-provider)");
    let is_valid = result.is_nil() || result.as_keyword().is_some();
    assert!(is_valid, "expected nil or keyword, got: {result}");
}

#[test]
fn test_llm_with_fallback_accepts_bare_and_override_entries() {
    // The chain may mix bare provider keywords with per-provider [provider model]
    // overrides and {:provider :model} maps. A body that doesn't call an LLM runs
    // without any provider configured, so this exercises chain parsing + execution.
    let result = eval(
        r#"(llm/with-fallback
              [:anthropic
               [:openai "gpt-5.5"]
               {:provider :groq :model "llama-3.3-70b-versatile"}]
              (lambda () 42))"#,
    );
    assert_eq!(result, Value::int(42));
}

#[test]
fn test_llm_with_fallback_rejects_malformed_entry() {
    let interp = Interpreter::new();
    // A 3-element vector is not a valid [provider model] pair.
    let result = interp.eval_str(r#"(llm/with-fallback [[:openai "a" "b"]] (lambda () 1))"#);
    assert!(result.is_err(), "expected error for malformed chain entry");
}

// --- Vector store tests ---

#[test]
fn test_vector_store_create() {
    let result = eval(r#"(vector-store/create "test-store")"#);
    assert_eq!(result, Value::string("test-store"));
}

#[test]
fn test_vector_store_count_empty() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "ct")"#).unwrap();
    assert_eq!(
        interp.eval_str(r#"(vector-store/count "ct")"#).unwrap(),
        Value::int(0)
    );
}

#[test]
fn test_vector_store_add_and_count() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "add-t")"#).unwrap();
    interp
        .eval_str(
            r#"(vector-store/add "add-t" "doc1" (embedding/list->embedding '(1.0 0.0 0.0)) {:title "Doc 1"})"#,
        )
        .unwrap();
    assert_eq!(
        interp.eval_str(r#"(vector-store/count "add-t")"#).unwrap(),
        Value::int(1)
    );
}

#[test]
fn test_vector_store_search() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "s-t")"#).unwrap();
    interp
        .eval_str(
            r#"(vector-store/add "s-t" "x" (embedding/list->embedding '(1.0 0.0 0.0)) {:axis "x"})"#,
        )
        .unwrap();
    interp
        .eval_str(
            r#"(vector-store/add "s-t" "y" (embedding/list->embedding '(0.0 1.0 0.0)) {:axis "y"})"#,
        )
        .unwrap();
    interp
        .eval_str(
            r#"(vector-store/add "s-t" "z" (embedding/list->embedding '(0.0 0.0 1.0)) {:axis "z"})"#,
        )
        .unwrap();
    let result = interp
        .eval_str(r#"(vector-store/search "s-t" (embedding/list->embedding '(0.9 0.1 0.0)) 1)"#)
        .unwrap();
    let results = result.as_list().unwrap();
    assert_eq!(results.len(), 1);
    let first = results[0].as_map_rc().unwrap();
    assert_eq!(
        first.get(&Value::keyword("id")).unwrap().as_str().unwrap(),
        "x"
    );
    let score = first
        .get(&Value::keyword("score"))
        .unwrap()
        .as_float()
        .unwrap();
    assert!(score > 0.9);
}

#[test]
fn test_vector_store_search_top_k() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "tk")"#).unwrap();
    interp
        .eval_str(r#"(vector-store/add "tk" "a" (embedding/list->embedding '(1.0 0.0)) {})"#)
        .unwrap();
    interp
        .eval_str(r#"(vector-store/add "tk" "b" (embedding/list->embedding '(0.9 0.1)) {})"#)
        .unwrap();
    interp
        .eval_str(r#"(vector-store/add "tk" "c" (embedding/list->embedding '(0.0 1.0)) {})"#)
        .unwrap();
    let result = interp
        .eval_str(r#"(vector-store/search "tk" (embedding/list->embedding '(1.0 0.0)) 2)"#)
        .unwrap();
    let results = result.as_list().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0]
            .as_map_rc()
            .unwrap()
            .get(&Value::keyword("id"))
            .unwrap()
            .as_str()
            .unwrap(),
        "a"
    );
}

#[test]
fn test_vector_store_delete() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "del")"#).unwrap();
    interp
        .eval_str(r#"(vector-store/add "del" "d1" (embedding/list->embedding '(1.0 0.0)) {})"#)
        .unwrap();
    interp
        .eval_str(r#"(vector-store/add "del" "d2" (embedding/list->embedding '(0.0 1.0)) {})"#)
        .unwrap();
    assert_eq!(
        interp
            .eval_str(r#"(vector-store/delete "del" "d1")"#)
            .unwrap(),
        Value::bool(true)
    );
    assert_eq!(
        interp.eval_str(r#"(vector-store/count "del")"#).unwrap(),
        Value::int(1)
    );
}

#[test]
fn test_vector_store_delete_nonexistent() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "dn")"#).unwrap();
    assert_eq!(
        interp
            .eval_str(r#"(vector-store/delete "dn" "nope")"#)
            .unwrap(),
        Value::bool(false)
    );
}

#[test]
fn test_vector_store_not_found() {
    let interp = Interpreter::new();
    assert!(interp
        .eval_str(r#"(vector-store/count "nonexistent")"#)
        .is_err());
}

#[test]
fn test_vector_store_save_and_open() {
    let tmp = std::env::temp_dir().join("sema-vs-test-save.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);

    // Create, add docs, save
    {
        let interp = Interpreter::new();
        interp.eval_str(r#"(vector-store/create "sv")"#).unwrap();
        interp
            .eval_str(
                r#"(vector-store/add "sv" "d1" (embedding/list->embedding '(1.0 0.0)) {:source "a.txt"})"#,
            )
            .unwrap();
        interp
            .eval_str(
                r#"(vector-store/add "sv" "d2" (embedding/list->embedding '(0.0 1.0)) {:source "b.txt"})"#,
            )
            .unwrap();
        interp
            .eval_str(&format!(r#"(vector-store/save "sv" "{path}")"#))
            .unwrap();
    }

    // Open from disk in a new interpreter
    {
        let interp = Interpreter::new();
        interp
            .eval_str(&format!(r#"(vector-store/open "loaded" "{path}")"#))
            .unwrap();
        assert_eq!(
            interp.eval_str(r#"(vector-store/count "loaded")"#).unwrap(),
            Value::int(2)
        );
        // Search should work on loaded store
        let result = interp
            .eval_str(r#"(vector-store/search "loaded" (embedding/list->embedding '(1.0 0.0)) 1)"#)
            .unwrap();
        let results = result.as_list().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0]
                .as_map_rc()
                .unwrap()
                .get(&Value::keyword("id"))
                .unwrap()
                .as_str()
                .unwrap(),
            "d1"
        );
    }
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_vector_store_open_nonexistent_creates_empty() {
    let tmp = std::env::temp_dir().join("sema-vs-test-open-new.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);

    let interp = Interpreter::new();
    interp
        .eval_str(&format!(r#"(vector-store/open "empty" "{path}")"#))
        .unwrap();
    assert_eq!(
        interp.eval_str(r#"(vector-store/count "empty")"#).unwrap(),
        Value::int(0)
    );
    // Save should work (path is associated)
    interp.eval_str(r#"(vector-store/save "empty")"#).unwrap();
    assert!(std::path::Path::new(path).exists());
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_vector_store_open_then_save_implicit_path() {
    let tmp = std::env::temp_dir().join("sema-vs-test-implicit.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);

    let interp = Interpreter::new();
    interp
        .eval_str(&format!(r#"(vector-store/open "imp" "{path}")"#))
        .unwrap();
    interp
        .eval_str(r#"(vector-store/add "imp" "x" (embedding/list->embedding '(1.0 2.0)) {})"#)
        .unwrap();
    // Save without explicit path — should use the path from open
    interp.eval_str(r#"(vector-store/save "imp")"#).unwrap();
    assert!(std::path::Path::new(path).exists());
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_vector_store_search_returns_metadata() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "mt")"#).unwrap();
    interp
        .eval_str(
            r#"(vector-store/add "mt" "d1" (embedding/list->embedding '(1.0 0.0)) {:source "f.txt" :page 3})"#,
        )
        .unwrap();
    let result = interp
        .eval_str(r#"(vector-store/search "mt" (embedding/list->embedding '(1.0 0.0)) 1)"#)
        .unwrap();
    let meta = result.as_list().unwrap()[0]
        .as_map_rc()
        .unwrap()
        .get(&Value::keyword("metadata"))
        .unwrap()
        .as_map_rc()
        .unwrap();
    assert_eq!(
        meta.get(&Value::keyword("source"))
            .unwrap()
            .as_str()
            .unwrap(),
        "f.txt"
    );
}

#[test]
fn test_vector_store_overwrite_id() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(vector-store/create "ow")"#).unwrap();
    interp
        .eval_str(r#"(vector-store/add "ow" "d1" (embedding/list->embedding '(1.0 0.0)) {:v 1})"#)
        .unwrap();
    interp
        .eval_str(r#"(vector-store/add "ow" "d1" (embedding/list->embedding '(0.0 1.0)) {:v 2})"#)
        .unwrap();
    assert_eq!(
        interp.eval_str(r#"(vector-store/count "ow")"#).unwrap(),
        Value::int(1)
    );
}

// --- Vector math tests ---

#[test]
fn test_vector_cosine_similarity() {
    let r = eval(
        r#"(vector/cosine-similarity (embedding/list->embedding '(1.0 0.0)) (embedding/list->embedding '(1.0 0.0)))"#,
    );
    assert!((r.as_float().unwrap() - 1.0).abs() < 1e-10);
}

#[test]
fn test_vector_cosine_orthogonal() {
    let r = eval(
        r#"(vector/cosine-similarity (embedding/list->embedding '(1.0 0.0)) (embedding/list->embedding '(0.0 1.0)))"#,
    );
    assert!(r.as_float().unwrap().abs() < 1e-10);
}

#[test]
fn test_vector_dot_product() {
    let r = eval(
        r#"(vector/dot-product (embedding/list->embedding '(1.0 2.0 3.0)) (embedding/list->embedding '(4.0 5.0 6.0)))"#,
    );
    assert!((r.as_float().unwrap() - 32.0).abs() < 1e-10);
}

#[test]
fn test_vector_normalize() {
    let r = eval(r#"(vector/normalize (embedding/list->embedding '(3.0 4.0)))"#);
    let bv = r.as_bytevector().unwrap();
    let x = f64::from_le_bytes(bv[0..8].try_into().unwrap());
    let y = f64::from_le_bytes(bv[8..16].try_into().unwrap());
    assert!((x - 0.6).abs() < 1e-10);
    assert!((y - 0.8).abs() < 1e-10);
}

#[test]
fn test_vector_normalize_zero() {
    let r = eval(r#"(vector/normalize (embedding/list->embedding '(0.0 0.0)))"#);
    let bv = r.as_bytevector().unwrap();
    assert!(f64::from_le_bytes(bv[0..8].try_into().unwrap()).abs() < 1e-10);
}

#[test]
fn test_vector_distance() {
    let r = eval(
        r#"(vector/distance (embedding/list->embedding '(0.0 0.0)) (embedding/list->embedding '(3.0 4.0)))"#,
    );
    assert!((r.as_float().unwrap() - 5.0).abs() < 1e-10);
}

#[test]
fn test_vector_distance_same() {
    let r = eval(
        r#"(vector/distance (embedding/list->embedding '(1.0 2.0)) (embedding/list->embedding '(1.0 2.0)))"#,
    );
    assert!(r.as_float().unwrap().abs() < 1e-10);
}

#[test]
// --- Text chunking tests ---

fn test_text_chunk_basic() {
    let result = eval(r#"(text/chunk "hello world foo bar" {:size 10})"#);
    let chunks = result.as_list().expect("should be a list");
    assert!(chunks.len() >= 2);
    for chunk in chunks {
        let s = chunk.as_str().expect("each chunk should be a string");
        assert!(s.len() <= 10, "chunk too long: '{s}' ({})", s.len());
    }
}

#[test]
fn test_text_chunk_default_size() {
    let result = eval(r#"(text/chunk "short text")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].as_str().unwrap(), "short text");
}

#[test]
fn test_text_chunk_with_overlap() {
    let result = eval(r#"(text/chunk "aaaa bbbb cccc dddd" {:size 10 :overlap 4})"#);
    let chunks = result.as_list().expect("should be a list");
    assert!(chunks.len() >= 2);
}

#[test]
fn test_text_chunk_empty() {
    let result = eval(r#"(text/chunk "")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 0);
}

#[test]
fn test_text_chunk_by_separator() {
    let result = eval(r#"(text/chunk-by-separator "a\nb\nc\nd" "\n")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 4);
    assert_eq!(chunks[0].as_str().unwrap(), "a");
}

#[test]
fn test_text_chunk_by_separator_empty() {
    let result = eval(r#"(text/chunk-by-separator "" "\n")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 0);
}

#[test]
fn test_text_split_sentences() {
    let result = eval(r#"(text/split-sentences "Hello world. How are you? I am fine.")"#);
    let chunks = result.as_list().expect("should be a list");
    assert_eq!(chunks.len(), 3);
}

#[test]
fn test_text_split_sentences_empty() {
    let result = eval(r#"(text/split-sentences "")"#);
    assert_eq!(result.as_list().unwrap().len(), 0);
}

#[test]
fn test_text_split_sentences_no_punctuation() {
    let result = eval(r#"(text/split-sentences "hello world")"#);
    let chunks = result.as_list().unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].as_str().unwrap(), "hello world");
}

// --- Text cleaning tests ---

#[test]
fn test_text_clean_whitespace() {
    assert_eq!(
        eval(r#"(text/clean-whitespace "  hello   world  \n\n  foo  ")"#),
        Value::string("hello world foo")
    );
}

#[test]
fn test_text_strip_html() {
    assert_eq!(
        eval(r#"(text/strip-html "<p>Hello <b>world</b></p>")"#),
        Value::string("Hello world")
    );
}

#[test]
fn test_text_strip_html_entities() {
    assert_eq!(
        eval(r#"(text/strip-html "a &amp; b &lt; c")"#),
        Value::string("a & b < c")
    );
}

#[test]
fn test_text_truncate_short() {
    assert_eq!(
        eval(r#"(text/truncate "hello" 10)"#),
        Value::string("hello")
    );
}

#[test]
fn test_text_truncate_exact() {
    assert_eq!(
        eval(r#"(text/truncate "hello world" 5)"#),
        Value::string("he...")
    );
}

#[test]
fn test_text_truncate_custom_suffix() {
    assert_eq!(
        eval(r#"(text/truncate "hello world" 8 "…")"#),
        Value::string("hello w…")
    );
}

#[test]
fn test_text_word_count() {
    assert_eq!(
        eval(r#"(text/word-count "hello world foo bar")"#),
        Value::int(4)
    );
}

#[test]
fn test_text_word_count_empty() {
    assert_eq!(eval(r#"(text/word-count "")"#), Value::int(0));
}

#[test]
fn test_text_word_count_extra_spaces() {
    assert_eq!(
        eval(r#"(text/word-count "  hello   world  ")"#),
        Value::int(2)
    );
}

#[test]
fn test_text_trim_indent() {
    assert_eq!(
        eval(r#"(text/trim-indent "    hello\n    world")"#),
        Value::string("hello\nworld")
    );
}

#[test]
fn test_text_trim_indent_mixed() {
    assert_eq!(
        eval(r#"(text/trim-indent "    hello\n      world")"#),
        Value::string("hello\n  world")
    );
}

#[test]
fn test_text_trim_indent_empty() {
    assert_eq!(eval(r#"(text/trim-indent "")"#), Value::string(""));
}

// --- Prompt template tests ---

#[test]
fn test_prompt_template_basic() {
    let result = eval(r#"(prompt/template "Hello {{name}}")"#);
    assert!(result.as_str().is_some());
}

#[test]
fn test_prompt_render_basic() {
    assert_eq!(
        eval(
            r#"(prompt/render "Hello {{name}}, welcome to {{place}}." {:name "Alice" :place "Wonderland"})"#
        ),
        Value::string("Hello Alice, welcome to Wonderland.")
    );
}

#[test]
fn test_prompt_render_missing_var() {
    assert_eq!(
        eval(r#"(prompt/render "Hello {{name}}, {{missing}}." {:name "Bob"})"#),
        Value::string("Hello Bob, {{missing}}.")
    );
}

#[test]
fn test_prompt_render_no_vars() {
    assert_eq!(
        eval(r#"(prompt/render "Hello world." {})"#),
        Value::string("Hello world.")
    );
}

#[test]
fn test_prompt_render_number_value() {
    assert_eq!(
        eval(r#"(prompt/render "Count: {{n}}" {:n 42})"#),
        Value::string("Count: 42")
    );
}

#[test]
fn test_prompt_render_repeated_var() {
    assert_eq!(
        eval(r#"(prompt/render "{{x}} and {{x}}" {:x "hello"})"#),
        Value::string("hello and hello")
    );
}

#[test]
fn test_prompt_render_adjacent_vars() {
    assert_eq!(
        eval(r#"(prompt/render "{{a}}{{b}}" {:a "hello" :b "world"})"#),
        Value::string("helloworld")
    );
}

// --- Token counting tests ---

#[test]
fn test_llm_token_count_basic() {
    let result = eval(r#"(llm/token-count "hello world")"#);
    let count = result.as_int().expect("should be integer");
    assert!((2..=4).contains(&count), "unexpected count: {count}");
}

#[test]
fn test_llm_token_count_empty() {
    assert_eq!(eval(r#"(llm/token-count "")"#), Value::int(0));
}

#[test]
fn test_llm_token_count_long() {
    let result = eval(
        r#"(llm/token-count (string-append "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd" "abcd"))"#,
    );
    assert_eq!(result.as_int().unwrap(), 10);
}

#[test]
fn test_llm_token_estimate_map() {
    let result = eval(r#"(llm/token-estimate "hello world")"#);
    let map = result.as_map_rc().expect("should be a map");
    assert!(map.contains_key(&Value::keyword("tokens")));
    assert!(map.contains_key(&Value::keyword("method")));
    assert_eq!(
        map.get(&Value::keyword("method"))
            .unwrap()
            .as_str()
            .unwrap(),
        "chars/4"
    );
}

#[test]
fn test_llm_token_count_list() {
    let result = eval(r#"(llm/token-count '("hello" "world" "foo"))"#);
    let count = result.as_int().expect("should be integer");
    assert!(count >= 3);
}

// --- Rate limiting tests ---

// TODO: `llm/with-rate-limit` has no dedicated test of its rate-limiting
// behavior. The previous `test_llm_with_rate_limit_type_check` only invoked
// the wrapper once and asserted the inner result, which exercised neither the
// interval enforcement nor any back-pressure path. A proper test would call
// the wrapped fn repeatedly in tight succession and assert that the elapsed
// `(time-ms)` between calls is >= the configured interval; that requires a
// timing-stable harness we don't yet have here, so the weak test has been
// removed rather than left as a false-positive.

// --- Retry tests ---

#[test]
fn test_retry_succeeds_first_try() {
    let result = eval(r#"(retry (lambda () 42))"#);
    assert_eq!(result, Value::int(42));
}

#[test]
fn test_retry_with_options() {
    let result = eval(r#"(retry (lambda () 42) {:max-attempts 3})"#);
    assert_eq!(result, Value::int(42));
}

#[test]
fn test_retry_counter() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"
        (begin
            (define counter 0)
            (retry (lambda ()
                (set! counter (+ counter 1))
                (if (< counter 3)
                    (error "not yet")
                    counter))
                {:max-attempts 5 :base-delay-ms 0}))
    "#,
        )
        .unwrap();
    assert_eq!(result, Value::int(3));
}

#[test]
fn test_retry_exhausted() {
    let interp = Interpreter::new();
    let result = interp.eval_str(
        r#"(retry (lambda () (error "always fails")) {:max-attempts 2 :base-delay-ms 0})"#,
    );
    assert!(result.is_err());
}

// --- LLM convenience tests ---

#[test]
fn test_llm_summarize_arity() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(llm/summarize)"#);
    assert!(result.is_err());
}

#[test]
fn test_llm_compare_arity() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(llm/compare "a")"#);
    assert!(result.is_err());
}

// --- KV store tests ---

#[test]
fn test_kv_open_and_close() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-oc.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "test" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/close "test")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_and_get() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-sg.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "sg" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "sg" "name" "Alice")"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "sg" "name")"#).unwrap();
    assert_eq!(result, Value::string("Alice"));
    interp.eval_str(r#"(kv/close "sg")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_get_missing() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-gm.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "gm" "{path}")"#))
        .unwrap();
    let result = interp.eval_str(r#"(kv/get "gm" "missing")"#).unwrap();
    assert!(result.is_nil());
    interp.eval_str(r#"(kv/close "gm")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_delete() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-del.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "del" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "del" "k" "v")"#).unwrap();
    interp.eval_str(r#"(kv/delete "del" "k")"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "del" "k")"#).unwrap();
    assert!(result.is_nil());
    interp.eval_str(r#"(kv/close "del")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_keys() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-keys.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "keys" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "keys" "a" 1)"#).unwrap();
    interp.eval_str(r#"(kv/set "keys" "b" 2)"#).unwrap();
    let result = interp.eval_str(r#"(kv/keys "keys")"#).unwrap();
    let keys = result.as_list().unwrap();
    assert_eq!(keys.len(), 2);
    interp.eval_str(r#"(kv/close "keys")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_persistence() {
    let tmp = std::env::temp_dir().join("sema-kv-test-persist.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    {
        let interp = Interpreter::new();
        interp
            .eval_str(&format!(r#"(kv/open "p" "{path}")"#))
            .unwrap();
        interp.eval_str(r#"(kv/set "p" "key" "value")"#).unwrap();
        interp.eval_str(r#"(kv/close "p")"#).unwrap();
    }
    {
        let interp = Interpreter::new();
        interp
            .eval_str(&format!(r#"(kv/open "p" "{path}")"#))
            .unwrap();
        let result = interp.eval_str(r#"(kv/get "p" "key")"#).unwrap();
        assert_eq!(result, Value::string("value"));
        interp.eval_str(r#"(kv/close "p")"#).unwrap();
    }
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_and_get_map() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-map.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "m" "{path}")"#))
        .unwrap();
    interp
        .eval_str(r#"(kv/set "m" "user" {:name "Alice" :age 30})"#)
        .unwrap();
    let result = interp.eval_str(r#"(kv/get "m" "user")"#).unwrap();
    let map = result.as_map_rc().expect("expected map back from kv/get");
    assert_eq!(
        map.get(&Value::keyword("name")).unwrap().as_str().unwrap(),
        "Alice"
    );
    assert_eq!(
        map.get(&Value::keyword("age")).unwrap().as_int().unwrap(),
        30
    );
    interp.eval_str(r#"(kv/close "m")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_and_get_list() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-list.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "l" "{path}")"#))
        .unwrap();
    interp
        .eval_str(r#"(kv/set "l" "nums" (list 1 2 3))"#)
        .unwrap();
    let result = interp.eval_str(r#"(kv/get "l" "nums")"#).unwrap();
    let items = result.as_list().expect("expected list back from kv/get");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].as_int().unwrap(), 1);
    interp.eval_str(r#"(kv/close "l")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_nested_map_with_nan() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-nan.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "n" "{path}")"#))
        .unwrap();
    // NaN inside a map — lossy conversion should preserve the map and null-out the NaN
    interp
        .eval_str(r#"(kv/set "n" "data" {:ok 42 :bad math/nan})"#)
        .unwrap();
    let result = interp.eval_str(r#"(kv/get "n" "data")"#).unwrap();
    let map = result
        .as_map_rc()
        .expect("expected map back from kv/get, not a string");
    assert_eq!(
        map.get(&Value::keyword("ok")).unwrap().as_int().unwrap(),
        42
    );
    assert!(
        map.get(&Value::keyword("bad")).unwrap().is_nil(),
        "NaN should round-trip through KV as nil"
    );
    interp.eval_str(r#"(kv/close "n")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_persistence_with_nested_data() {
    let tmp = std::env::temp_dir().join("sema-kv-test-nested-persist.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    {
        let interp = Interpreter::new();
        interp
            .eval_str(&format!(r#"(kv/open "np" "{path}")"#))
            .unwrap();
        interp
            .eval_str(
                r#"(kv/set "np" "config" {:host "localhost" :port 8080 :tags (list "a" "b")})"#,
            )
            .unwrap();
        interp.eval_str(r#"(kv/close "np")"#).unwrap();
    }
    {
        let interp = Interpreter::new();
        interp
            .eval_str(&format!(r#"(kv/open "np" "{path}")"#))
            .unwrap();
        let result = interp.eval_str(r#"(kv/get "np" "config")"#).unwrap();
        let map = result
            .as_map_rc()
            .expect("expected persisted map from kv/get");
        assert_eq!(
            map.get(&Value::keyword("host")).unwrap().as_str().unwrap(),
            "localhost"
        );
        assert_eq!(
            map.get(&Value::keyword("port")).unwrap().as_int().unwrap(),
            8080
        );
        let tags = map
            .get(&Value::keyword("tags"))
            .unwrap()
            .as_list()
            .expect("tags should be a list");
        assert_eq!(tags.len(), 2);
        interp.eval_str(r#"(kv/close "np")"#).unwrap();
    }
    let _ = std::fs::remove_file(&tmp);
}

// --- Document metadata tests ---

#[test]
fn test_document_create() {
    let result = eval(r#"(document/create "hello world" {:source "test.txt"})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(
        map.get(&Value::keyword("text")).unwrap().as_str().unwrap(),
        "hello world"
    );
    let meta = map
        .get(&Value::keyword("metadata"))
        .unwrap()
        .as_map_rc()
        .unwrap();
    assert_eq!(
        meta.get(&Value::keyword("source"))
            .unwrap()
            .as_str()
            .unwrap(),
        "test.txt"
    );
}

#[test]
fn test_document_chunk_preserves_metadata() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"
        (document/chunk
            (document/create "aaaa bbbb cccc dddd" {:source "test.txt" :page 1})
            {:size 10})
    "#,
        )
        .unwrap();
    let chunks = result.as_list().unwrap();
    assert!(chunks.len() >= 2);
    for chunk in chunks {
        let map = chunk.as_map_rc().expect("chunk should be a map");
        assert!(map.get(&Value::keyword("text")).unwrap().as_str().is_some());
        let meta = map
            .get(&Value::keyword("metadata"))
            .unwrap()
            .as_map_rc()
            .unwrap();
        assert_eq!(
            meta.get(&Value::keyword("source"))
                .unwrap()
                .as_str()
                .unwrap(),
            "test.txt"
        );
    }
}

#[test]
fn test_document_chunk_adds_chunk_index() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"
        (document/chunk
            (document/create "aaaa bbbb cccc dddd" {:source "f.txt"})
            {:size 10})
    "#,
        )
        .unwrap();
    let chunks = result.as_list().unwrap();
    for (i, chunk) in chunks.iter().enumerate() {
        let meta = chunk
            .as_map_rc()
            .unwrap()
            .get(&Value::keyword("metadata"))
            .unwrap()
            .as_map_rc()
            .unwrap();
        assert_eq!(
            meta.get(&Value::keyword("chunk-index"))
                .unwrap()
                .as_int()
                .unwrap(),
            i as i64
        );
    }
}

#[test]
fn test_document_text() {
    let result = eval(r#"(document/text (document/create "hello" {:source "x"}))"#);
    assert_eq!(result, Value::string("hello"));
}

#[test]
fn test_document_metadata() {
    let result = eval(r#"(document/metadata (document/create "hello" {:source "x"}))"#);
    let meta = result.as_map_rc().unwrap();
    assert_eq!(
        meta.get(&Value::keyword("source"))
            .unwrap()
            .as_str()
            .unwrap(),
        "x"
    );
}

#[test]
fn test_vector_dimension_mismatch_error() {
    let interp = Interpreter::new();
    assert!(interp
        .eval_str(
            r#"(vector/dot-product (embedding/list->embedding '(1.0 2.0)) (embedding/list->embedding '(1.0 2.0 3.0)))"#,
        )
        .is_err());
}

#[test]
fn test_map_zip() {
    assert_eq!(
        eval_to_string("(map/entries (map/zip (list :a :b :c) (list 1 2 3)))"),
        "((:a 1) (:b 2) (:c 3))"
    );
    // Uneven lists - shorter wins
    assert_eq!(
        eval("(count (map/zip (list :a :b) (list 1 2 3)))"),
        Value::int(2)
    );
    // Empty
    assert_eq!(eval("(count (map/zip (list) (list)))"), Value::int(0));
}

#[test]
fn test_context_set_get() {
    assert_eq!(
        eval(r#"(begin (context/set :name "alice") (context/get :name))"#),
        Value::string("alice")
    );
    assert_eq!(eval("(context/get :missing)"), Value::nil());
}

#[test]
fn test_context_has() {
    assert_eq!(
        eval("(begin (context/set :x 1) (context/has? :x))"),
        Value::bool(true)
    );
    assert_eq!(eval("(context/has? :nope)"), Value::bool(false));
}

#[test]
fn test_context_remove() {
    assert_eq!(
        eval("(begin (context/set :x 1) (context/remove :x) (context/has? :x))"),
        Value::bool(false)
    );
}

#[test]
fn test_context_all() {
    let result = eval("(begin (context/set :a 1) (context/set :b 2) (context/all))");
    let map = result.as_map_rc().expect("should be a map");
    assert_eq!(map.get(&Value::keyword("a")), Some(&Value::int(1)));
    assert_eq!(map.get(&Value::keyword("b")), Some(&Value::int(2)));
}

#[test]
fn test_context_pull() {
    assert_eq!(
        eval(
            r#"(begin (context/set :temp "value") (define pulled (context/pull :temp)) (list pulled (context/has? :temp)))"#
        ),
        eval(r#"(list "value" #f)"#),
    );
}

#[test]
fn test_context_with_scoped() {
    assert_eq!(
        eval(
            r#"(begin
            (context/set :x "outer")
            (context/with {:x "inner" :y "only-inner"}
                (lambda () (list (context/get :x) (context/get :y)))))"#
        ),
        eval(r#"(list "inner" "only-inner")"#),
    );
    // After context/with, :x is restored and :y is gone
    assert_eq!(
        eval(
            r#"(begin
            (context/set :x "outer")
            (context/with {:x "inner"} (lambda () nil))
            (list (context/get :x) (context/get :y)))"#
        ),
        eval(r#"(list "outer" nil)"#),
    );
}

#[test]
fn test_context_with_nested() {
    assert_eq!(
        eval(
            r#"(begin
            (context/set :a 1)
            (context/with {:b 2}
                (lambda ()
                    (context/with {:c 3}
                        (lambda ()
                            (list (context/get :a) (context/get :b) (context/get :c)))))))"#
        ),
        eval("(list 1 2 3)"),
    );
}

#[test]
fn test_context_hidden() {
    assert_eq!(
        eval(
            r#"(begin
            (context/set-hidden :secret "s3cret")
            (list (context/get-hidden :secret) (context/get :secret)))"#
        ),
        eval(r#"(list "s3cret" nil)"#),
    );
}

#[test]
fn test_context_hidden_not_in_all() {
    let result = eval(
        r#"(begin
        (context/set :visible 1)
        (context/set-hidden :invisible 2)
        (context/all))"#,
    );
    let map = result.as_map_rc().expect("should be map");
    assert_eq!(map.get(&Value::keyword("visible")), Some(&Value::int(1)));
    assert_eq!(map.get(&Value::keyword("invisible")), None);
}

#[test]
fn test_context_has_hidden() {
    assert_eq!(
        eval(r#"(begin (context/set-hidden :k "v") (context/has-hidden? :k))"#),
        Value::bool(true),
    );
    assert_eq!(eval("(context/has-hidden? :nope)"), Value::bool(false));
}

#[test]
fn test_context_stack_push_get() {
    assert_eq!(
        eval(
            r#"(begin
            (context/push :breadcrumbs "first")
            (context/push :breadcrumbs "second")
            (context/push :breadcrumbs "third")
            (context/stack :breadcrumbs))"#
        ),
        eval(r#"(list "first" "second" "third")"#),
    );
}

#[test]
fn test_context_stack_pop() {
    assert_eq!(
        eval(
            r#"(begin
            (context/push :trail "a")
            (context/push :trail "b")
            (context/pop :trail))"#
        ),
        Value::string("b"),
    );
    assert_eq!(
        eval(
            r#"(begin
            (context/push :trail "a")
            (context/push :trail "b")
            (context/pop :trail)
            (context/stack :trail))"#
        ),
        eval(r#"(list "a")"#),
    );
}

#[test]
fn test_context_stack_empty() {
    assert_eq!(eval("(context/stack :empty)"), eval("(list)"));
    assert_eq!(eval("(context/pop :empty)"), Value::nil());
}

#[test]
fn test_context_merge() {
    assert_eq!(
        eval(
            r#"(begin
            (context/set :a 1)
            (context/merge {:b 2 :c 3})
            (list (context/get :a) (context/get :b) (context/get :c)))"#
        ),
        eval("(list 1 2 3)"),
    );
}

#[test]
fn test_context_clear() {
    assert_eq!(
        eval(
            r#"(begin
            (context/set :a 1)
            (context/set :b 2)
            (context/clear)
            (context/all))"#
        ),
        eval("{}"),
    );
}

#[test]
fn test_log_includes_context() {
    eval(
        r#"(begin
        (context/set :trace-id "abc-123")
        (context/set :user-id 42)
        (log/info "test message"))"#,
    );
}

#[test]
fn test_log_functions_basic() {
    eval(r#"(log/info "hello")"#);
    eval(r#"(log/warn "caution")"#);
    eval(r#"(log/error "problem")"#);
    eval(r#"(log/debug "details")"#);
}

#[test]
fn test_context_with_restores_on_error() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(context/set :x "before")"#).unwrap();
    let _ = interp.eval_str(r#"(context/with {:x "during"} (lambda () (error "boom")))"#);
    assert_eq!(
        interp.eval_str(r#"(context/get :x)"#).unwrap(),
        Value::string("before"),
    );
}

#[test]
fn test_context_with_any_value_types() {
    assert_eq!(
        eval(
            r#"(begin
            (context/set "string-key" 42)
            (context/set 123 "number-key")
            (list (context/get "string-key") (context/get 123)))"#
        ),
        eval(r#"(list 42 "number-key")"#),
    );
}

#[test]
fn test_context_stacks_independent() {
    assert_eq!(
        eval(
            r#"(begin
            (context/push :a 1)
            (context/push :b 2)
            (list (context/stack :a) (context/stack :b)))"#
        ),
        eval("(list (list 1) (list 2))"),
    );
}

// --- Arity error tests ---

#[test]
fn test_context_arity_errors() {
    assert_arity_error("(context/set :x)");
    assert_arity_error("(context/set :x 1 2)");
    assert_arity_error("(context/get)");
    assert_arity_error("(context/get :a :b)");
    assert_arity_error("(context/has?)");
    assert_arity_error("(context/remove)");
    assert_arity_error("(context/all :x)");
    assert_arity_error("(context/pull)");
    assert_arity_error("(context/push :x)");
    assert_arity_error("(context/stack)");
    assert_arity_error("(context/pop)");
    assert_arity_error("(context/merge)");
    assert_arity_error("(context/clear :x)");
    assert_arity_error("(context/with {:a 1})");
    assert_arity_error("(context/set-hidden :x)");
    assert_arity_error("(context/get-hidden)");
    assert_arity_error("(context/has-hidden?)");
}

// --- Type error tests ---

#[test]
fn test_context_type_errors() {
    let err = eval_err(r#"(context/with "not-a-map" (lambda () nil))"#);
    assert!(matches!(err.inner(), SemaError::Type { .. }));
    let err = eval_err(r#"(context/with {:a 1} "not-a-function")"#);
    assert!(matches!(err.inner(), SemaError::Type { .. }));
    let err = eval_err(r#"(context/merge "not-a-map")"#);
    assert!(matches!(err.inner(), SemaError::Type { .. }));
}

// --- Overwrite and return value tests ---

#[test]
fn test_context_set_overwrites() {
    assert_eq!(
        eval("(begin (context/set :x 1) (context/set :x 2) (context/get :x))"),
        Value::int(2),
    );
}

#[test]
fn test_context_remove_returns_value() {
    assert_eq!(
        eval(r#"(begin (context/set :x "hello") (context/remove :x))"#),
        Value::string("hello"),
    );
    assert_eq!(eval("(context/remove :missing)"), Value::nil());
}

#[test]
fn test_context_merge_overwrites() {
    assert_eq!(
        eval("(begin (context/set :a 1) (context/merge {:a 99 :b 2}) (context/get :a))"),
        Value::int(99),
    );
}

// --- Scoping edge cases ---

#[test]
fn test_context_set_inside_with_does_not_persist() {
    let interp = Interpreter::new();
    interp
        .eval_str(r#"(context/with {:x 1} (lambda () (context/set :new-key "inner")))"#)
        .unwrap();
    assert_eq!(
        interp.eval_str("(context/get :new-key)").unwrap(),
        Value::nil(),
    );
}

#[test]
fn test_context_all_merges_scoped_frames() {
    assert_eq!(
        eval(
            r#"(begin
            (context/set :a 1)
            (context/with {:b 2 :a 99}
                (lambda () (context/all))))"#
        ),
        eval("{:a 99 :b 2}"),
    );
}

#[test]
fn test_context_with_empty_map() {
    assert_eq!(
        eval("(begin (context/set :x 1) (context/with {} (lambda () (context/get :x))))"),
        Value::int(1),
    );
}

#[test]
fn test_context_stacks_persist_through_with() {
    let interp = Interpreter::new();
    interp
        .eval_str(r#"(context/with {:x 1} (lambda () (context/push :trail "inside")))"#)
        .unwrap();
    assert_eq!(
        interp.eval_str("(context/stack :trail)").unwrap(),
        eval(r#"(list "inside")"#),
    );
}

#[test]
fn test_context_hidden_unaffected_by_with() {
    let interp = Interpreter::new();
    interp
        .eval_str(r#"(context/set-hidden :secret "val")"#)
        .unwrap();
    interp
        .eval_str(r#"(context/with {:x 1} (lambda () nil))"#)
        .unwrap();
    assert_eq!(
        interp.eval_str(r#"(context/get-hidden :secret)"#).unwrap(),
        Value::string("val"),
    );
}

#[test]
fn test_context_remove_across_frames() {
    let interp = Interpreter::new();
    interp.eval_str("(context/set :x 1)").unwrap();
    let result = interp
        .eval_str(
            r#"(context/with {:x 2}
            (lambda ()
                (context/remove :x)
                (context/get :x)))"#,
        )
        .unwrap();
    assert_eq!(result, Value::nil());
    assert_eq!(interp.eval_str("(context/get :x)").unwrap(), Value::nil(),);
}

// ── String boundary slicers ──

#[test]
fn test_string_after() {
    assert_eq!(
        eval(r#"(string/after "This is my name" "This is ")"#),
        Value::string("my name")
    );
    assert_eq!(
        eval(r#"(string/after "hello" "missing")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/after "one::two::three" "::")"#),
        Value::string("two::three")
    );
    // edge: empty input, empty needle, boundary positions, unicode
    assert_eq!(eval(r#"(string/after "" "x")"#), Value::string(""));
    assert_eq!(eval(r#"(string/after "hello" "")"#), Value::string("hello"));
    assert_eq!(eval(r#"(string/after "abc" "a")"#), Value::string("bc"));
    assert_eq!(eval(r#"(string/after "abc" "c")"#), Value::string(""));
    assert_eq!(eval(r#"(string/after "abc" "abc")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/after "café🎉bar" "🎉")"#),
        Value::string("bar")
    );
    assert_eq!(
        eval(r#"(string/after "ééé hannah" "han")"#),
        Value::string("nah")
    );
}

#[test]
fn test_string_after_last() {
    assert_eq!(
        eval(r#"(string/after-last "one::two::three" "::")"#),
        Value::string("three")
    );
    assert_eq!(
        eval(r#"(string/after-last "hello" "missing")"#),
        Value::string("hello")
    );
    // edge: empty needle, boundary positions, unicode
    assert_eq!(eval(r#"(string/after-last "" "x")"#), Value::string(""));
    assert_eq!(eval(r#"(string/after-last "hello" "")"#), Value::string(""));
    assert_eq!(eval(r#"(string/after-last "abc" "c")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/after-last "abc" "abc")"#),
        Value::string("")
    );
    assert_eq!(
        eval(r#"(string/after-last "yvette" "t")"#),
        Value::string("e")
    );
    assert_eq!(
        eval(r#"(string/after-last "yvette" "tte")"#),
        Value::string("")
    );
    assert_eq!(
        eval(r#"(string/after-last "寿司🍣寿司🍣end" "🍣")"#),
        Value::string("end")
    );
}

#[test]
fn test_string_before() {
    assert_eq!(
        eval(r#"(string/before "This is my name" " my")"#),
        Value::string("This is")
    );
    assert_eq!(
        eval(r#"(string/before "hello" "missing")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/before "one::two::three" "::")"#),
        Value::string("one")
    );
    // edge: empty input, empty needle, boundary positions, unicode
    assert_eq!(eval(r#"(string/before "" "x")"#), Value::string(""));
    assert_eq!(eval(r#"(string/before "hello" "")"#), Value::string(""));
    assert_eq!(eval(r#"(string/before "abc" "a")"#), Value::string(""));
    assert_eq!(eval(r#"(string/before "abc" "c")"#), Value::string("ab"));
    assert_eq!(eval(r#"(string/before "abc" "abc")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/before "café🎉bar" "🎉")"#),
        Value::string("café")
    );
    assert_eq!(
        eval(r#"(string/before "foo@bar.com" "@")"#),
        Value::string("foo")
    );
    assert_eq!(
        eval(r#"(string/before "@foo@bar.com" "@")"#),
        Value::string("")
    );
}

#[test]
fn test_string_before_last() {
    assert_eq!(
        eval(r#"(string/before-last "one::two::three" "::")"#),
        Value::string("one::two")
    );
    assert_eq!(
        eval(r#"(string/before-last "hello" "missing")"#),
        Value::string("hello")
    );
    // edge: empty needle, boundary positions, unicode
    assert_eq!(eval(r#"(string/before-last "" "x")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/before-last "hello" "")"#),
        Value::string("hello")
    );
    assert_eq!(eval(r#"(string/before-last "abc" "a")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/before-last "abc" "abc")"#),
        Value::string("")
    );
    assert_eq!(
        eval(r#"(string/before-last "yvette" "t")"#),
        Value::string("yvet")
    );
    assert_eq!(
        eval(r#"(string/before-last "寿司🍣寿司🍣end" "🍣")"#),
        Value::string("寿司🍣寿司")
    );
    assert_eq!(
        eval(r#"(string/before-last "laravel framework" " ")"#),
        Value::string("laravel")
    );
}

#[test]
fn test_string_between() {
    assert_eq!(
        eval(r#"(string/between "This is my name" "This " " name")"#),
        Value::string("is my")
    );
    assert_eq!(
        eval(r#"(string/between "[hello]" "[" "]")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/between "no match" "(" ")")"#),
        Value::string("")
    );
    // edge: empty delimiters, nested brackets, unicode
    assert_eq!(eval(r#"(string/between "" "[" "]")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/between "abc" "" "c")"#),
        Value::string("ab")
    );
    assert_eq!(eval(r#"(string/between "abc" "a" "")"#), Value::string(""));
    assert_eq!(eval(r#"(string/between "abc" "" "")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/between "hannah" "ha" "ah")"#),
        Value::string("nn")
    );
    assert_eq!(
        eval(r#"(string/between "[a]ab[b]" "[" "]")"#),
        Value::string("a")
    );
    assert_eq!(
        eval(r#"(string/between "foobarbar" "foo" "bar")"#),
        Value::string("")
    );
    assert_eq!(
        eval(r#"(string/between "寿司(🍣)定食" "(" ")")"#),
        Value::string("🍣")
    );
}

// ── Prefix/suffix tools ──

#[test]
fn test_string_chop_start() {
    assert_eq!(
        eval(r#"(string/chop-start "Hello World" "Hello ")"#),
        Value::string("World")
    );
    assert_eq!(
        eval(r#"(string/chop-start "Hello" "Bye")"#),
        Value::string("Hello")
    );
    // edge: empty string, empty prefix, unicode/emoji
    assert_eq!(eval(r#"(string/chop-start "" "x")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/chop-start "hello" "")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/chop-start "http://laravel.com" "http://")"#),
        Value::string("laravel.com")
    );
    assert_eq!(
        eval(r#"(string/chop-start "🌊✋" "🌊")"#),
        Value::string("✋")
    );
    assert_eq!(
        eval(r#"(string/chop-start "🌊✋" "✋")"#),
        Value::string("🌊✋")
    );
    assert_eq!(
        eval(r#"(string/chop-start "こんにちは世界" "こんにちは")"#),
        Value::string("世界")
    );
}

#[test]
fn test_string_chop_end() {
    assert_eq!(
        eval(r#"(string/chop-end "Hello World" " World")"#),
        Value::string("Hello")
    );
    assert_eq!(
        eval(r#"(string/chop-end "Hello" "Bye")"#),
        Value::string("Hello")
    );
    // edge: empty string, empty suffix, unicode/emoji
    assert_eq!(eval(r#"(string/chop-end "" "x")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/chop-end "hello" "")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/chop-end "path/to/file.php" ".php")"#),
        Value::string("path/to/file")
    );
    assert_eq!(
        eval(r#"(string/chop-end "✋🌊" "🌊")"#),
        Value::string("✋")
    );
    assert_eq!(
        eval(r#"(string/chop-end "✋🌊" "✋")"#),
        Value::string("✋🌊")
    );
    assert_eq!(
        eval(r#"(string/chop-end "寿司🍣" "🍣")"#),
        Value::string("寿司")
    );
}

#[test]
fn test_string_ensure_start() {
    assert_eq!(
        eval(r#"(string/ensure-start "world" "hello ")"#),
        Value::string("hello world")
    );
    assert_eq!(
        eval(r#"(string/ensure-start "hello world" "hello ")"#),
        Value::string("hello world")
    );
    // edge: empty input, empty prefix
    assert_eq!(
        eval(r#"(string/ensure-start "" "pre-")"#),
        Value::string("pre-")
    );
    assert_eq!(
        eval(r#"(string/ensure-start "hello" "")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/ensure-start "test/string" "/")"#),
        Value::string("/test/string")
    );
    assert_eq!(
        eval(r#"(string/ensure-start "/test/string" "/")"#),
        Value::string("/test/string")
    );
}

#[test]
fn test_string_ensure_end() {
    assert_eq!(
        eval(r#"(string/ensure-end "/path" "/")"#),
        Value::string("/path/")
    );
    assert_eq!(
        eval(r#"(string/ensure-end "/path/" "/")"#),
        Value::string("/path/")
    );
    // edge: empty input, empty suffix
    assert_eq!(
        eval(r#"(string/ensure-end "" "-post")"#),
        Value::string("-post")
    );
    assert_eq!(
        eval(r#"(string/ensure-end "hello" "")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/ensure-end "ab" "bc")"#),
        Value::string("abbc")
    );
}

// ── Replace variants ──

#[test]
fn test_string_replace_first() {
    assert_eq!(
        eval(r#"(string/replace-first "aaa" "a" "b")"#),
        Value::string("baa")
    );
    assert_eq!(
        eval(r#"(string/replace-first "hello" "x" "y")"#),
        Value::string("hello")
    );
    // edge: empty input, empty needle, unicode
    assert_eq!(
        eval(r#"(string/replace-first "" "a" "b")"#),
        Value::string("")
    );
    assert_eq!(
        eval(r#"(string/replace-first "hello" "" "X")"#),
        Value::string("Xhello")
    );
    assert_eq!(
        eval(r#"(string/replace-first "🍣🍣" "🍣" "x")"#),
        Value::string("x🍣")
    );
    assert_eq!(
        eval(r#"(string/replace-first "foobar foobar" "bar" "qux")"#),
        Value::string("fooqux foobar")
    );
}

#[test]
fn test_string_replace_last() {
    assert_eq!(
        eval(r#"(string/replace-last "aaa" "a" "b")"#),
        Value::string("aab")
    );
    assert_eq!(
        eval(r#"(string/replace-last "hello" "x" "y")"#),
        Value::string("hello")
    );
    // edge: empty input, empty needle, unicode
    assert_eq!(
        eval(r#"(string/replace-last "" "a" "b")"#),
        Value::string("")
    );
    assert_eq!(
        eval(r#"(string/replace-last "hello" "" "X")"#),
        Value::string("helloX")
    );
    assert_eq!(
        eval(r#"(string/replace-last "🍣🍣" "🍣" "x")"#),
        Value::string("🍣x")
    );
    assert_eq!(
        eval(r#"(string/replace-last "foobar foobar" "bar" "qux")"#),
        Value::string("foobar fooqux")
    );
}

#[test]
fn test_string_remove() {
    assert_eq!(
        eval(r#"(string/remove "hello world" "o")"#),
        Value::string("hell wrld")
    );
    assert_eq!(eval(r#"(string/remove "abc" "x")"#), Value::string("abc"));
    // edge: empty input, empty needle, multi-byte removal
    assert_eq!(eval(r#"(string/remove "" "x")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/remove "hello" "")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/remove "Foobar" "o")"#),
        Value::string("Fbar")
    );
    assert_eq!(
        eval(r#"(string/remove "café café" "é")"#),
        Value::string("caf caf")
    );
}

#[test]
fn test_string_take() {
    assert_eq!(eval(r#"(string/take "hello" 3)"#), Value::string("hel"));
    assert_eq!(eval(r#"(string/take "hello" -3)"#), Value::string("llo"));
    assert_eq!(eval(r#"(string/take "hi" 10)"#), Value::string("hi"));
    assert_eq!(eval(r#"(string/take "hello" 0)"#), Value::string(""));
    // edge: empty string, unicode/multibyte
    assert_eq!(eval(r#"(string/take "" 3)"#), Value::string(""));
    assert_eq!(eval(r#"(string/take "🎉🎉" 1)"#), Value::string("🎉"));
    assert_eq!(eval(r#"(string/take "🎉🎉" -1)"#), Value::string("🎉"));
    assert_eq!(eval(r#"(string/take "寿司" 1)"#), Value::string("寿"));
    assert_eq!(eval(r#"(string/take "寿司" -1)"#), Value::string("司"));
    assert_eq!(eval(r#"(string/take "abcdef" 6)"#), Value::string("abcdef"));
}

// ── Identifier casing ──

#[test]
fn test_string_snake_case() {
    assert_eq!(
        eval(r#"(string/snake-case "helloWorld")"#),
        Value::string("hello_world")
    );
    assert_eq!(
        eval(r#"(string/snake-case "HelloWorld")"#),
        Value::string("hello_world")
    );
    assert_eq!(
        eval(r#"(string/snake-case "hello-world")"#),
        Value::string("hello_world")
    );
    assert_eq!(
        eval(r#"(string/snake-case "Hello World")"#),
        Value::string("hello_world")
    );
    // edge: empty, consecutive separators, acronyms, numbers, unicode
    assert_eq!(eval(r#"(string/snake-case "")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/snake-case "foo--bar")"#),
        Value::string("foo_bar")
    );
    assert_eq!(
        eval(r#"(string/snake-case "foo__bar")"#),
        Value::string("foo_bar")
    );
    assert_eq!(
        eval(r#"(string/snake-case "HTMLParser")"#),
        Value::string("html_parser")
    );
    assert_eq!(
        eval(r#"(string/snake-case "LaravelPHPFramework")"#),
        Value::string("laravel_php_framework")
    );
    assert_eq!(
        eval(r#"(string/snake-case "user2FAEnabled")"#),
        Value::string("user2_fa_enabled")
    );
    assert_eq!(
        eval(r#"(string/snake-case "CaféConLeche")"#),
        Value::string("café_con_leche")
    );
}

#[test]
fn test_string_kebab_case() {
    assert_eq!(
        eval(r#"(string/kebab-case "helloWorld")"#),
        Value::string("hello-world")
    );
    assert_eq!(
        eval(r#"(string/kebab-case "hello_world")"#),
        Value::string("hello-world")
    );
    assert_eq!(
        eval(r#"(string/kebab-case "HelloWorld")"#),
        Value::string("hello-world")
    );
    // edge: empty, consecutive separators, acronyms
    assert_eq!(eval(r#"(string/kebab-case "")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/kebab-case "foo__bar")"#),
        Value::string("foo-bar")
    );
    assert_eq!(
        eval(r#"(string/kebab-case "HTMLParser")"#),
        Value::string("html-parser")
    );
    assert_eq!(
        eval(r#"(string/kebab-case "user2FAEnabled")"#),
        Value::string("user2-fa-enabled")
    );
}

#[test]
fn test_string_camel_case() {
    assert_eq!(
        eval(r#"(string/camel-case "hello_world")"#),
        Value::string("helloWorld")
    );
    assert_eq!(
        eval(r#"(string/camel-case "hello-world")"#),
        Value::string("helloWorld")
    );
    assert_eq!(
        eval(r#"(string/camel-case "Hello World")"#),
        Value::string("helloWorld")
    );
    // edge: empty, consecutive separators, acronyms, numbers
    assert_eq!(eval(r#"(string/camel-case "")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/camel-case "foo__bar")"#),
        Value::string("fooBar")
    );
    assert_eq!(
        eval(r#"(string/camel-case "HTMLParser")"#),
        Value::string("htmlParser")
    );
    assert_eq!(
        eval(r#"(string/camel-case "FooBar")"#),
        Value::string("fooBar")
    );
}

#[test]
fn test_string_pascal_case() {
    assert_eq!(
        eval(r#"(string/pascal-case "hello_world")"#),
        Value::string("HelloWorld")
    );
    assert_eq!(
        eval(r#"(string/pascal-case "hello-world")"#),
        Value::string("HelloWorld")
    );
    assert_eq!(
        eval(r#"(string/pascal-case "hello world")"#),
        Value::string("HelloWorld")
    );
    // edge: empty, consecutive separators, acronyms
    assert_eq!(eval(r#"(string/pascal-case "")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/pascal-case "foo__bar")"#),
        Value::string("FooBar")
    );
    assert_eq!(
        eval(r#"(string/pascal-case "HTMLParser")"#),
        Value::string("HtmlParser")
    );
}

#[test]
fn test_string_headline() {
    assert_eq!(
        eval(r#"(string/headline "hello_world")"#),
        Value::string("Hello World")
    );
    assert_eq!(
        eval(r#"(string/headline "helloWorld")"#),
        Value::string("Hello World")
    );
    assert_eq!(
        eval(r#"(string/headline "hello-world")"#),
        Value::string("Hello World")
    );
    // edge: empty, consecutive/mixed separators, acronyms
    assert_eq!(eval(r#"(string/headline "")"#), Value::string(""));
    assert_eq!(
        eval(r#"(string/headline "foo--bar__baz")"#),
        Value::string("Foo Bar Baz")
    );
    assert_eq!(
        eval(r#"(string/headline "HTMLParser")"#),
        Value::string("Html Parser")
    );
    assert_eq!(
        eval(r#"(string/headline "LaravelPHPFramework")"#),
        Value::string("Laravel Php Framework")
    );
    assert_eq!(
        eval(r#"(string/headline "user2FAEnabled")"#),
        Value::string("User2 Fa Enabled")
    );
}

#[test]
fn test_string_words() {
    assert_eq!(
        eval(r#"(string/words "helloWorld")"#),
        eval(r#"'("hello" "World")"#)
    );
    assert_eq!(
        eval(r#"(string/words "hello_world")"#),
        eval(r#"'("hello" "world")"#)
    );
    // edge: consecutive separators, acronyms
    assert_eq!(
        eval(r#"(string/words "foo--bar")"#),
        eval(r#"'("foo" "bar")"#)
    );
    assert_eq!(
        eval(r#"(string/words "HTMLParser")"#),
        eval(r#"'("HTML" "Parser")"#)
    );
    assert_eq!(
        eval(r#"(string/words "LaravelPHPFramework")"#),
        eval(r#"'("Laravel" "PHP" "Framework")"#)
    );
    assert_eq!(
        eval(r#"(string/words "user2FAEnabled")"#),
        eval(r#"'("user2" "FA" "Enabled")"#)
    );
}

// ── Wrap/unwrap ──

#[test]
fn test_string_wrap() {
    assert_eq!(
        eval(r#"(string/wrap "hello" "'")"#),
        Value::string("'hello'")
    );
    assert_eq!(
        eval(r#"(string/wrap "hello" "[" "]")"#),
        Value::string("[hello]")
    );
    // edge: empty delimiter, empty string, unicode
    assert_eq!(eval(r#"(string/wrap "hello" "")"#), Value::string("hello"));
    assert_eq!(eval(r#"(string/wrap "" "'")"#), Value::string("''"));
    assert_eq!(eval(r#"(string/wrap "X" "🧪")"#), Value::string("🧪X🧪"));
    assert_eq!(eval(r#"(string/wrap "値" "«" "»")"#), Value::string("«値»"));
    assert_eq!(
        eval(r#"(string/wrap "-bar-" "foo" "baz")"#),
        Value::string("foo-bar-baz")
    );
}

#[test]
fn test_string_unwrap() {
    assert_eq!(
        eval(r#"(string/unwrap "'hello'" "'")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/unwrap "[hello]" "[" "]")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/unwrap "hello" "'")"#),
        Value::string("hello")
    );
    // edge: only one side matches (Sema leaves unchanged), asymmetric delimiters
    assert_eq!(
        eval(r#"(string/unwrap "'hello" "'")"#),
        Value::string("'hello")
    );
    assert_eq!(
        eval(r#"(string/unwrap "hello'" "'")"#),
        Value::string("hello'")
    );
    assert_eq!(
        eval(r#"(string/unwrap "[hello" "[" "]")"#),
        Value::string("[hello")
    );
    assert_eq!(
        eval(r#"(string/unwrap "hello]" "[" "]")"#),
        Value::string("hello]")
    );
    assert_eq!(
        eval(r#"(string/unwrap "foo-bar-baz" "foo-" "-baz")"#),
        Value::string("bar")
    );
    assert_eq!(
        eval(r#"(string/unwrap "{some: \"json\"}" "{" "}")"#),
        Value::string("some: \"json\"")
    );
}

// ── Text helpers ──

#[test]
fn test_text_excerpt() {
    assert_eq!(
        eval(r#"(text/excerpt "This is my name" "my" {:radius 3})"#),
        Value::string("...is my na...")
    );
    assert_eq!(
        eval(r#"(text/excerpt "This is my name" "name" {:radius 3 :omission "(...) "})"#),
        Value::string("(...) my name")
    );
    assert_eq!(eval(r#"(text/excerpt "hello" "missing")"#), Value::nil());
    assert_eq!(
        eval(r#"(text/excerpt "short" "short")"#),
        Value::string("short")
    );
    // edge: case-insensitive matching, unicode, empty input
    assert_eq!(
        eval(r#"(text/excerpt "Hello World" "WORLD" {:radius 0})"#),
        Value::string("...World")
    );
    assert_eq!(
        eval(r#"(text/excerpt "Hello World" "world" {:radius 0})"#),
        Value::string("...World")
    );
    assert_eq!(
        eval(r#"(text/excerpt "naïve café" "CAFÉ" {:radius 0})"#),
        Value::string("...café")
    );
    assert_eq!(eval(r#"(text/excerpt "" "x")"#), Value::nil());
    assert_eq!(
        eval(r#"(text/excerpt "" "" {:radius 0})"#),
        Value::string("")
    );
}

#[test]
fn test_text_normalize_newlines() {
    assert_eq!(
        eval(r#"(text/normalize-newlines "a\r\nb\rc")"#),
        Value::string("a\nb\nc")
    );
    assert_eq!(
        eval(r#"(text/normalize-newlines "already\nfine")"#),
        Value::string("already\nfine")
    );
}

// ── Deep edge case tests ──

#[test]
fn test_string_after_multi_byte_needle() {
    // multi-byte needle within multi-byte string
    assert_eq!(
        eval(r#"(string/after "ééé hannah" "han")"#),
        Value::string("nah")
    );
    // newline as needle
    assert_eq!(
        eval(r#"(string/after "line1\nline2" "\n")"#),
        Value::string("line2")
    );
    // needle appears multiple times — returns after first
    assert_eq!(eval(r#"(string/after "a.b.c" ".")"#), Value::string("b.c"));
}

#[test]
fn test_string_between_left_found_right_missing() {
    // left found, right not found → returns everything after left
    assert_eq!(
        eval(r#"(string/between "foo left only" "foo" "bar")"#),
        Value::string(" left only")
    );
    // left & right adjacent → empty result
    assert_eq!(
        eval(r#"(string/between "foobar" "foo" "bar")"#),
        Value::string("")
    );
    // left & right with content between
    assert_eq!(
        eval(r#"(string/between "fooxbar" "foo" "bar")"#),
        Value::string("x")
    );
    // multi-char delimiters
    assert_eq!(
        eval(r#"(string/between "123456789" "123" "6789")"#),
        Value::string("45")
    );
}

#[test]
fn test_string_unwrap_minimal_length() {
    // exactly delimiter + delimiter with empty content
    assert_eq!(eval(r#"(string/unwrap "xx" "x")"#), Value::string(""));
    // single char can't be unwrapped with single-char delimiter (len < left+right)
    assert_eq!(eval(r#"(string/unwrap "x" "x")"#), Value::string("x"));
    // multi-char delimiters wrapping empty content
    assert_eq!(eval(r#"(string/unwrap "[]" "[" "]")"#), Value::string(""));
    // delimiter longer than string
    assert_eq!(eval(r#"(string/unwrap "ab" "abc")"#), Value::string("ab"));
}

#[test]
fn test_string_replace_first_full_and_delete() {
    // replace entire string
    assert_eq!(
        eval(r#"(string/replace-first "abc" "abc" "xyz")"#),
        Value::string("xyz")
    );
    // replace with empty (delete first occurrence)
    assert_eq!(
        eval(r#"(string/replace-first "abc" "b" "")"#),
        Value::string("ac")
    );
    // multi-byte replacement
    assert_eq!(
        eval(r#"(string/replace-first "Jönköping Malmö" "Jö" "xxx")"#),
        Value::string("xxxnköping Malmö")
    );
}

#[test]
fn test_string_replace_last_full_and_delete() {
    // replace entire string
    assert_eq!(
        eval(r#"(string/replace-last "abc" "abc" "xyz")"#),
        Value::string("xyz")
    );
    // replace with empty (delete last occurrence)
    assert_eq!(
        eval(r#"(string/replace-last "abcb" "b" "")"#),
        Value::string("abc")
    );
    // multi-byte replacement
    assert_eq!(
        eval(r#"(string/replace-last "Malmö Jönköping" "öping" "yyy")"#),
        Value::string("Malmö Jönkyyy")
    );
}

#[test]
fn test_string_remove_multi_char_and_overlapping() {
    // remove multi-char substring
    assert_eq!(
        eval(r#"(string/remove "Foobar" "bar")"#),
        Value::string("Foo")
    );
    assert_eq!(
        eval(r#"(string/remove "Foobar" "F")"#),
        Value::string("oobar")
    );
    // remove multiple occurrences
    assert_eq!(eval(r#"(string/remove "abcabc" "abc")"#), Value::string(""));
    // case sensitive
    assert_eq!(
        eval(r#"(string/remove "Foobar" "f")"#),
        Value::string("Foobar")
    );
}

#[test]
fn test_string_take_exact_length() {
    // take exactly the string length
    assert_eq!(eval(r#"(string/take "abc" 3)"#), Value::string("abc"));
    // negative take exceeding length returns full string
    assert_eq!(eval(r#"(string/take "ab" -10)"#), Value::string("ab"));
    // single char string
    assert_eq!(eval(r#"(string/take "a" 1)"#), Value::string("a"));
    assert_eq!(eval(r#"(string/take "a" -1)"#), Value::string("a"));
    // multi-byte: take from string with mixed ascii and emoji
    assert_eq!(eval(r#"(string/take "a🎉b" 2)"#), Value::string("a🎉"));
    assert_eq!(eval(r#"(string/take "a🎉b" -2)"#), Value::string("🎉b"));
}

#[test]
fn test_string_snake_case_idempotent_and_edge() {
    // already in snake_case
    assert_eq!(
        eval(r#"(string/snake-case "already_snake")"#),
        Value::string("already_snake")
    );
    // single uppercase char
    assert_eq!(eval(r#"(string/snake-case "A")"#), Value::string("a"));
    // all uppercase (acronym)
    assert_eq!(eval(r#"(string/snake-case "ABC")"#), Value::string("abc"));
    // acronym then lowercase
    assert_eq!(
        eval(r#"(string/snake-case "ABCDef")"#),
        Value::string("abc_def")
    );
    // dot-separated (namespace-like)
    assert_eq!(
        eval(r#"(string/snake-case "foo.bar.baz")"#),
        Value::string("foo_bar_baz")
    );
    // multi-acronym
    assert_eq!(
        eval(r#"(string/snake-case "getHTTPSUrl")"#),
        Value::string("get_https_url")
    );
    assert_eq!(
        eval(r#"(string/snake-case "XMLToJSON")"#),
        Value::string("xml_to_json")
    );
    // trailing number
    assert_eq!(
        eval(r#"(string/snake-case "version2")"#),
        Value::string("version2")
    );
}

#[test]
fn test_string_camel_case_idempotent_and_edge() {
    // already camelCase — idempotent
    assert_eq!(
        eval(r#"(string/camel-case "alreadyCamel")"#),
        Value::string("alreadyCamel")
    );
    // single word
    assert_eq!(
        eval(r#"(string/camel-case "hello")"#),
        Value::string("hello")
    );
    // all uppercase words
    assert_eq!(
        eval(r#"(string/camel-case "FOO_BAR")"#),
        Value::string("fooBar")
    );
    // dot-separated
    assert_eq!(
        eval(r#"(string/camel-case "foo.bar.baz")"#),
        Value::string("fooBarBaz")
    );
}

#[test]
fn test_string_pascal_case_edge() {
    // single word
    assert_eq!(
        eval(r#"(string/pascal-case "hello")"#),
        Value::string("Hello")
    );
    // all uppercase words
    assert_eq!(
        eval(r#"(string/pascal-case "FOO_BAR")"#),
        Value::string("FooBar")
    );
    // dot-separated
    assert_eq!(
        eval(r#"(string/pascal-case "foo.bar.baz")"#),
        Value::string("FooBarBaz")
    );
}

#[test]
fn test_string_headline_unicode() {
    // unicode uppercase transitions (Laravel-inspired)
    assert_eq!(
        eval(r#"(string/headline "sindÖdeUndSo")"#),
        Value::string("Sind Öde Und So")
    );
    assert_eq!(
        eval(r#"(string/headline "öffentliche-überraschungen")"#),
        Value::string("Öffentliche Überraschungen")
    );
}

#[test]
fn test_string_words_separators_only() {
    // all separators → empty list
    assert_eq!(eval(r#"(string/words "")"#), eval("'()"));
    assert_eq!(eval(r#"(string/words "___")"#), eval("'()"));
    assert_eq!(eval(r#"(string/words "---")"#), eval("'()"));
    assert_eq!(eval(r#"(string/words "   ")"#), eval("'()"));
    // single word (no separators)
    assert_eq!(eval(r#"(string/words "abc")"#), eval(r#"'("abc")"#));
    // trailing number stays with word
    assert_eq!(
        eval(r#"(string/words "version2")"#),
        eval(r#"'("version2")"#)
    );
    // multi-acronym
    assert_eq!(
        eval(r#"(string/words "getHTTPSUrl")"#),
        eval(r#"'("get" "HTTPS" "Url")"#)
    );
    assert_eq!(
        eval(r#"(string/words "XMLToJSON")"#),
        eval(r#"'("XML" "To" "JSON")"#)
    );
    // dot-separated
    assert_eq!(
        eval(r#"(string/words "foo.bar.baz")"#),
        eval(r#"'("foo" "bar" "baz")"#)
    );
}

#[test]
fn test_string_wrap_unwrap_roundtrip() {
    // wrap then unwrap should recover original
    assert_eq!(
        eval(r#"(string/unwrap (string/wrap "hello" "[" "]") "[" "]")"#),
        Value::string("hello")
    );
    assert_eq!(
        eval(r#"(string/unwrap (string/wrap "data" "<<" ">>") "<<" ">>")"#),
        Value::string("data")
    );
    // wrap with multi-char delimiter
    assert_eq!(
        eval(r#"(string/wrap "mid" "[]")"#),
        Value::string("[]mid[]")
    );
    assert_eq!(
        eval(r#"(string/unwrap "[]mid[]" "[]")"#),
        Value::string("mid")
    );
}

#[test]
fn test_text_excerpt_radius_zero() {
    // radius 0 — only the query itself plus omission markers
    assert_eq!(
        eval(r#"(text/excerpt "abcdef" "cde" {:radius 0})"#),
        Value::string("...cde...")
    );
    // query at start — no leading omission
    assert_eq!(
        eval(r#"(text/excerpt "abcdef" "abc" {:radius 0})"#),
        Value::string("abc...")
    );
    // query at end — no trailing omission
    assert_eq!(
        eval(r#"(text/excerpt "abcdef" "def" {:radius 0})"#),
        Value::string("...def")
    );
}

#[test]
fn test_text_excerpt_unicode_deep() {
    // CJK with radius (from Laravel tests)
    assert_eq!(
        eval(r#"(text/excerpt "㏗༼㏗" "༼" {:radius 0})"#),
        Value::string("...༼...")
    );
    // accented characters
    assert_eq!(
        eval(r#"(text/excerpt "Como você está" "ê" {:radius 2})"#),
        Value::string("...ocê e...")
    );
    // case-insensitive match on accented
    assert_eq!(
        eval(r#"(text/excerpt "João Antônio" "JOÃO" {:radius 5})"#),
        Value::string("João Antô...")
    );
    // default radius (100) on short string — no omission markers
    assert_eq!(
        eval(r#"(text/excerpt "short text" "text")"#),
        Value::string("short text")
    );
}

#[test]
fn test_text_normalize_newlines_edge() {
    // empty string
    assert_eq!(eval(r#"(text/normalize-newlines "")"#), Value::string(""));
    // only carriage returns
    assert_eq!(
        eval(r#"(text/normalize-newlines "\r\r\r")"#),
        Value::string("\n\n\n")
    );
    // mixed line endings
    assert_eq!(
        eval(r#"(text/normalize-newlines "a\r\nb\rc\nd")"#),
        Value::string("a\nb\nc\nd")
    );
    // no line endings
    assert_eq!(
        eval(r#"(text/normalize-newlines "no newlines")"#),
        Value::string("no newlines")
    );
}

#[test]
fn test_string_chop_start_prefix_is_full_string() {
    // prefix equals entire string → empty result
    assert_eq!(
        eval(r#"(string/chop-start "hello" "hello")"#),
        Value::string("")
    );
    // suffix equals entire string → empty result
    assert_eq!(
        eval(r#"(string/chop-end "hello" "hello")"#),
        Value::string("")
    );
}

#[test]
fn test_string_ensure_start_already_doubled() {
    // ensure-start with prefix already present twice — should not add
    assert_eq!(
        eval(r#"(string/ensure-start "//test" "/")"#),
        Value::string("//test")
    );
    // ensure-end with suffix already present — should not add
    assert_eq!(
        eval(r#"(string/ensure-end "testbc" "bc")"#),
        Value::string("testbc")
    );
}

#[test]
fn test_string_between_overlapping_delimiters() {
    // same delimiter for left and right
    assert_eq!(
        eval(r#"(string/between "xhellox" "x" "x")"#),
        Value::string("hello")
    );
    // delimiter appears three times — takes first and next
    assert_eq!(
        eval(r#"(string/between "|a|b|c" "|" "|")"#),
        Value::string("a")
    );
}

#[test]
fn test_string_after_before_composability() {
    // composing after and before to extract between
    assert_eq!(
        eval(r#"(string/before (string/after "user@host.com" "@") ".")"#),
        Value::string("host")
    );
    // chop-start then chop-end to extract path
    assert_eq!(
        eval(
            r#"(string/chop-end (string/chop-start "http://example.com/path" "http://") "/path")"#
        ),
        Value::string("example.com")
    );
}

#[test]
fn test_casing_roundtrip() {
    // snake → camel → snake roundtrip
    assert_eq!(
        eval(r#"(string/snake-case (string/camel-case "hello_world"))"#),
        Value::string("hello_world")
    );
    // snake → pascal → snake roundtrip
    assert_eq!(
        eval(r#"(string/snake-case (string/pascal-case "hello_world"))"#),
        Value::string("hello_world")
    );
    // kebab → camel → kebab roundtrip
    assert_eq!(
        eval(r#"(string/kebab-case (string/camel-case "hello-world"))"#),
        Value::string("hello-world")
    );
}

// --- PDF processing tests ---

fn pdf_fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn test_pdf_extract_text() {
    let path = pdf_fixture("sample-invoice.pdf");
    let result = eval(&format!(r#"(pdf/extract-text "{path}")"#));
    let text = result.as_str().expect("should return a string");
    assert!(
        text.contains("Invoice"),
        "should contain 'Invoice', got: {text}"
    );
    assert!(text.contains("Acme"), "should contain 'Acme', got: {text}");
}

#[test]
fn test_pdf_extract_text_not_receipt() {
    let path = pdf_fixture("not-a-receipt.pdf");
    let result = eval(&format!(r#"(pdf/extract-text "{path}")"#));
    let text = result.as_str().expect("should return a string");
    assert!(
        text.contains("Meeting"),
        "should contain 'Meeting', got: {text}"
    );
    assert!(
        !text.contains("Invoice"),
        "should NOT contain 'Invoice', got: {text}"
    );
}

#[test]
fn test_pdf_extract_text_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/extract-text "/nonexistent/file.pdf")"#);
    assert!(result.is_err(), "should error on nonexistent file");
}

#[test]
fn test_pdf_extract_text_arity() {
    let interp = Interpreter::new();
    assert!(interp.eval_str(r#"(pdf/extract-text)"#).is_err());
}

#[test]
fn test_pdf_extract_text_pages() {
    let path = pdf_fixture("sample-invoice.pdf");
    let result = eval(&format!(r#"(pdf/extract-text-pages "{path}")"#));
    let pages = result.as_list().expect("should return a list");
    assert_eq!(pages.len(), 1, "single-page PDF should return 1 page");
    let page_text = pages[0].as_str().expect("page should be a string");
    assert!(
        page_text.contains("Invoice"),
        "page should contain 'Invoice'"
    );
}

#[test]
fn test_pdf_extract_text_pages_returns_list() {
    let path = pdf_fixture("sample-invoice.pdf");
    let result = eval(&format!(r#"(length (pdf/extract-text-pages "{path}"))"#));
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_extract_text_pages_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/extract-text-pages "/nonexistent.pdf")"#);
    assert!(result.is_err());
}

#[test]
fn test_pdf_page_count() {
    let path = pdf_fixture("sample-invoice.pdf");
    let result = eval(&format!(r#"(pdf/page-count "{path}")"#));
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_page_count_second_fixture() {
    let path = pdf_fixture("not-a-receipt.pdf");
    let result = eval(&format!(r#"(pdf/page-count "{path}")"#));
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_page_count_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/page-count "/nonexistent.pdf")"#);
    assert!(result.is_err());
}

#[test]
fn test_pdf_page_count_arity() {
    let interp = Interpreter::new();
    assert!(interp.eval_str(r#"(pdf/page-count)"#).is_err());
    assert!(interp.eval_str(r#"(pdf/page-count "a" "b")"#).is_err());
}

// `test_pdf_metadata_returns_map` was removed: it only asserted that the
// return value was a map, which is subsumed by `test_pdf_metadata_has_pages`
// below (that test does a `(get ... :pages)`, which itself requires a map
// AND verifies the expected field is present with the right value).

#[test]
fn test_pdf_metadata_has_pages() {
    let path = pdf_fixture("sample-invoice.pdf");
    let result = eval(&format!(r#"(get (pdf/metadata "{path}") :pages)"#));
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_pdf_metadata_has_title() {
    let path = pdf_fixture("sample-invoice.pdf");
    let result = eval(&format!(r#"(get (pdf/metadata "{path}") :title)"#));
    let title = result.as_str().expect("should have :title");
    assert_eq!(title, "Test Document");
}

#[test]
fn test_pdf_metadata_has_author() {
    let path = pdf_fixture("sample-invoice.pdf");
    let result = eval(&format!(r#"(get (pdf/metadata "{path}") :author)"#));
    let author = result.as_str().expect("should have :author");
    assert_eq!(author, "Sema Test Suite");
}

#[test]
fn test_pdf_metadata_nonexistent() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(pdf/metadata "/nonexistent.pdf")"#);
    assert!(result.is_err());
}

#[test]
fn test_pdf_metadata_arity() {
    let interp = Interpreter::new();
    assert!(interp.eval_str(r#"(pdf/metadata)"#).is_err());
    assert!(interp.eval_str(r#"(pdf/metadata "a" "b")"#).is_err());
}

// ── Pretty-print ──────────────────────────────────────────────────

#[test]
fn test_pretty_print_small_map_stays_compact() {
    let val = eval("{:a 1 :b 2}");
    assert_eq!(sema_core::pretty_print(&val, 80), "{:a 1 :b 2}");
}

#[test]
fn test_pretty_print_small_vector_stays_compact() {
    let val = eval("[1 2 3 4 5]");
    assert_eq!(sema_core::pretty_print(&val, 80), "[1 2 3 4 5]");
}

#[test]
fn test_pretty_print_small_list_stays_compact() {
    let val = eval(r#"(list {:id "doc-1" :score 0.92} {:id "doc-2" :score 0.85})"#);
    assert_eq!(
        sema_core::pretty_print(&val, 80),
        r#"({:id "doc-1" :score 0.92} {:id "doc-2" :score 0.85})"#
    );
}

#[test]
fn test_pretty_print_list_breaks_at_narrow_width() {
    let val = eval(r#"(list {:id "doc-1" :score 0.92} {:id "doc-2" :score 0.85})"#);
    assert_eq!(
        sema_core::pretty_print(&val, 40),
        "({:id \"doc-1\" :score 0.92}\n {:id \"doc-2\" :score 0.85})"
    );
}

#[test]
fn test_pretty_print_map_breaks_at_narrow_width() {
    let val =
        eval(r#"{:user "helge" :settings {:theme "dark" :font-size 14} :scores [95 87 92 88]}"#);
    assert_eq!(
        sema_core::pretty_print(&val, 60),
        "{:scores [95 87 92 88]\n :settings {:font-size 14 :theme \"dark\"}\n :user \"helge\"}"
    );
}

#[test]
fn test_pretty_print_vector_of_maps_breaks() {
    let val = eval(
        r#"[{:name "alice" :age 30 :city "oslo"} {:name "bob" :age 25 :city "berlin"} {:name "carol" :age 35 :city "paris"}]"#,
    );
    assert_eq!(
        sema_core::pretty_print(&val, 80),
        "[{:age 30 :city \"oslo\" :name \"alice\"}\n {:age 25 :city \"berlin\" :name \"bob\"}\n {:age 35 :city \"paris\" :name \"carol\"}]"
    );
}

#[test]
fn test_pretty_print_nested_list_of_maps() {
    let val = eval(
        r#"(list {:id "doc-1" :metadata {:source "greeting.txt"} :score 0.92} {:id "doc-2" :metadata {:source "readme.md"} :score 0.85})"#,
    );
    assert_eq!(
        sema_core::pretty_print(&val, 80),
        "({:id \"doc-1\" :metadata {:source \"greeting.txt\"} :score 0.92}\n {:id \"doc-2\" :metadata {:source \"readme.md\"} :score 0.85})"
    );
}

#[test]
fn test_pretty_print_nested_map_breaks_with_indent() {
    let val = eval(
        r#"{:headers (hash-map :Accept "application/json" :Host "httpbin.org" :User-Agent "Mozilla/5.0 (Macintosh)")}"#,
    );
    assert_eq!(
        sema_core::pretty_print(&val, 60),
        "{:headers\n   {:Accept \"application/json\"\n    :Host \"httpbin.org\"\n    :User-Agent \"Mozilla/5.0 (Macintosh)\"}}"
    );
}

#[test]
fn test_pprint_returns_nil() {
    assert_eq!(eval(r#"(pprint {:a 1})"#), Value::nil());
}

#[test]
fn test_pprint_arity() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(pprint)").is_err());
    assert!(interp.eval_str("(pprint 1 2)").is_err());
}

// ── Bytecode serialization CLI tests ──────────────────────────────

#[test]
fn test_compile_subcommand() {
    let dir = std::env::temp_dir().join("sema_test_compile");
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("test.sema");
    std::fs::write(&src, "(define x 42)").unwrap();

    let output = sema_cmd()
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let semac = dir.join("test.semac");
    assert!(semac.exists());

    // Verify magic number
    let bytes = std::fs::read(&semac).unwrap();
    assert_eq!(&bytes[0..4], b"\x00SEM");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_run_semac_file() {
    let dir = std::env::temp_dir().join("sema_test_run_semac");
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("hello.sema");
    std::fs::write(&src, r#"(println "hello from bytecode")"#).unwrap();

    // Compile
    let output = sema_cmd()
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());

    // Run the .semac file (auto-detected)
    let semac = dir.join("hello.semac");
    let output = sema_cmd().arg(semac.to_str().unwrap()).output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("hello from bytecode"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A `.semac` program that uses async must run to completion: the bytecode
/// `run_bytecode_bytes` path initializes the async scheduler before executing,
/// so `(await (async ...))` resolves instead of erroring with "no async
/// scheduler registered".
#[test]
fn test_run_semac_file_async() {
    let dir = std::env::temp_dir().join("sema_test_run_semac_async");
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("async.sema");
    std::fs::write(&src, r#"(println (await (async (+ 1 2))))"#).unwrap();

    // Compile to a .semac
    let output = sema_cmd()
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run the .semac file (auto-detected). Without the scheduler init this
    // would fail with "no async scheduler registered".
    let semac = dir.join("async.semac");
    let output = sema_cmd().arg(semac.to_str().unwrap()).output().unwrap();
    assert!(
        output.status.success(),
        "running async .semac failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains('3'),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_disasm_subcommand() {
    let dir = std::env::temp_dir().join("sema_test_disasm");
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("dis.sema");
    std::fs::write(&src, "(+ 1 2)").unwrap();

    let _ = sema_cmd()
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();

    let semac = dir.join("dis.semac");
    let output = sema_cmd()
        .args(["disasm", semac.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("CONST") || stdout.contains("RETURN"),
        "disasm output: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_bytecode_file_end_to_end() {
    let dir = std::env::temp_dir().join("sema_test_e2e");
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("e2e.sema");
    std::fs::write(
        &src,
        r#"
        (define (make-adder n)
          (lambda (x) (+ n x)))
        (define add5 (make-adder 5))
        (println (add5 10))
    "#,
    )
    .unwrap();

    // Compile
    let output = sema_cmd()
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run from bytecode
    let semac = dir.join("e2e.semac");
    let output = sema_cmd().arg(semac.to_str().unwrap()).output().unwrap();
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("15"),
        "expected 15, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// --- JSON additional coverage ---

#[test]
fn test_json_encode_vector() {
    assert_eq!(eval(r#"(json/encode [1 2 3])"#), Value::string("[1,2,3]"));
}

#[test]
fn test_json_encode_nested() {
    assert_eq!(
        eval(r#"(json/encode {:items [1 2] :meta {:ok #t}})"#),
        Value::string(r#"{"items":[1,2],"meta":{"ok":true}}"#)
    );
}

#[test]
fn test_json_encode_nil() {
    assert_eq!(eval(r#"(json/encode nil)"#), Value::string("null"));
}

#[test]
fn test_json_encode_booleans() {
    assert_eq!(eval(r#"(json/encode #t)"#), Value::string("true"));
    assert_eq!(eval(r#"(json/encode #f)"#), Value::string("false"));
}

#[test]
fn test_json_encode_keywords() {
    assert_eq!(eval(r#"(json/encode :hello)"#), Value::string(r#""hello""#));
}

#[test]
fn test_json_encode_list() {
    assert_eq!(
        eval(r#"(json/encode (list "a" "b" "c"))"#),
        Value::string(r#"["a","b","c"]"#)
    );
}

#[test]
fn test_json_decode_array() {
    assert_eq!(
        eval(r#"(json/decode "[1, 2, 3]")"#),
        Value::list(vec![Value::int(1), Value::int(2), Value::int(3)])
    );
}

#[test]
fn test_json_decode_null() {
    assert_eq!(eval(r#"(json/decode "null")"#), Value::nil());
}

#[test]
fn test_json_decode_boolean() {
    assert_eq!(eval(r#"(json/decode "true")"#), Value::bool(true));
    assert_eq!(eval(r#"(json/decode "false")"#), Value::bool(false));
}

#[test]
fn test_json_decode_string() {
    assert_eq!(eval(r#"(json/decode "\"hello\"")"#), Value::string("hello"));
}

#[test]
fn test_json_decode_number() {
    assert_eq!(eval(r#"(json/decode "42")"#), Value::int(42));
    assert_eq!(eval(r#"(json/decode "3.14")"#), Value::float(3.14));
}

#[test]
fn test_json_roundtrip_map() {
    let result = eval(r#"(get (json/decode (json/encode {:x 10 :y 20})) :x)"#);
    assert_eq!(result, Value::int(10));
}

#[test]
fn test_json_roundtrip_vector() {
    let result = eval(r#"(length (json/decode (json/encode [1 2 3 4 5])))"#);
    assert_eq!(result, Value::int(5));
}

#[test]
fn test_json_roundtrip_nested() {
    let result = eval(r#"(get (get (json/decode (json/encode {:a {:b 99}})) :a) :b)"#);
    assert_eq!(result, Value::int(99));
}

#[test]
fn test_json_encode_pretty_has_indentation() {
    let result = eval(r#"(json/encode-pretty {:x 1})"#);
    let s = result.as_str().unwrap();
    assert!(s.contains("  "), "expected indentation in pretty output");
    assert!(s.contains('\n'), "expected newlines in pretty output");
    assert!(s.contains("\"x\": 1"));
}

#[test]
fn test_json_encode_float() {
    assert_eq!(eval(r#"(json/encode 3.14)"#), Value::string("3.14"));
}

#[test]
fn test_json_encode_integer() {
    assert_eq!(eval(r#"(json/encode 42)"#), Value::string("42"));
}

#[test]
fn test_json_encode_string() {
    assert_eq!(
        eval(r#"(json/encode "hello")"#),
        Value::string(r#""hello""#)
    );
}

// --- KV additional coverage ---

#[test]
fn test_kv_delete_returns_bool() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-delbool.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "db" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "db" "k" "v")"#).unwrap();
    let existed = interp.eval_str(r#"(kv/delete "db" "k")"#).unwrap();
    assert_eq!(existed, Value::bool(true));
    let not_existed = interp.eval_str(r#"(kv/delete "db" "k")"#).unwrap();
    assert_eq!(not_existed, Value::bool(false));
    interp.eval_str(r#"(kv/close "db")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_numeric_value() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-num.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "num" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "num" "count" 42)"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "num" "count")"#).unwrap();
    assert_eq!(result, Value::int(42));
    interp.eval_str(r#"(kv/close "num")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_boolean_value() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-bool.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "bv" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "bv" "flag" #t)"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "bv" "flag")"#).unwrap();
    assert_eq!(result, Value::bool(true));
    interp.eval_str(r#"(kv/close "bv")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_set_nil_value() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-nil.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "nv" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "nv" "empty" nil)"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "nv" "empty")"#).unwrap();
    assert!(result.is_nil());
    interp.eval_str(r#"(kv/close "nv")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_keys_empty() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-kempty.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "ek" "{path}")"#))
        .unwrap();
    let result = interp.eval_str(r#"(kv/keys "ek")"#).unwrap();
    let keys = result.as_list().unwrap();
    assert_eq!(keys.len(), 0);
    interp.eval_str(r#"(kv/close "ek")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_overwrite_value() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-overwrite.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "ow" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "ow" "k" "old")"#).unwrap();
    interp.eval_str(r#"(kv/set "ow" "k" "new")"#).unwrap();
    let result = interp.eval_str(r#"(kv/get "ow" "k")"#).unwrap();
    assert_eq!(result, Value::string("new"));
    interp.eval_str(r#"(kv/close "ow")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_kv_open_returns_name() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-ret.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    let result = interp
        .eval_str(&format!(r#"(kv/open "mystore" "{path}")"#))
        .unwrap();
    assert_eq!(result, Value::string("mystore"));
    interp.eval_str(r#"(kv/close "mystore")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

// --- Bytevector additional coverage ---

#[test]
fn test_bv_copy_with_start_only() {
    assert_eq!(
        eval("(bytevector-copy #u8(10 20 30 40 50) 2)"),
        Value::bytevector(vec![30, 40, 50])
    );
}

#[test]
fn test_bv_copy_full() {
    assert_eq!(
        eval("(bytevector-copy #u8(1 2 3))"),
        Value::bytevector(vec![1, 2, 3])
    );
}

#[test]
fn test_bv_append_empty() {
    assert_eq!(eval("(bytevector-append)"), Value::bytevector(vec![]));
}

#[test]
fn test_bv_append_single() {
    assert_eq!(
        eval("(bytevector-append #u8(5 6 7))"),
        Value::bytevector(vec![5, 6, 7])
    );
}

#[test]
fn test_bv_list_roundtrip() {
    assert_eq!(
        eval("(list->bytevector (bytevector->list #u8(100 200 255)))"),
        Value::bytevector(vec![100, 200, 255])
    );
}

#[test]
fn test_bv_make_zero_size() {
    assert_eq!(eval("(make-bytevector 0)"), Value::bytevector(vec![]));
}

#[test]
fn test_bv_make_with_fill() {
    assert_eq!(
        eval("(make-bytevector 5 255)"),
        Value::bytevector(vec![255, 255, 255, 255, 255])
    );
}

#[test]
fn test_bv_u8_set_returns_new() {
    assert_eq!(
        eval("(bytevector-u8-ref (bytevector-u8-set! #u8(0 0 0) 1 42) 1)"),
        Value::int(42)
    );
}

#[test]
fn test_bv_u8_set_does_not_mutate_original() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(
            r#"(begin
                (define orig #u8(1 2 3))
                (define copy (bytevector-u8-set! orig 0 99))
                (bytevector-u8-ref orig 0))"#,
        )
        .unwrap();
    assert_eq!(result, Value::int(1));
}

#[test]
fn test_bv_copy_empty_range() {
    assert_eq!(
        eval("(bytevector-copy #u8(1 2 3) 1 1)"),
        Value::bytevector(vec![])
    );
}

#[test]
fn test_bv_bytevector_to_list_empty() {
    assert_eq!(eval("(bytevector->list #u8())"), Value::list(vec![]));
}

#[test]
fn test_bv_list_to_bytevector_empty() {
    assert_eq!(eval("(list->bytevector (list))"), Value::bytevector(vec![]));
}

// --- JSON additional coverage (decode object, encode map) ---

#[test]
fn test_json_encode_map() {
    let result = eval(r#"(json/encode {:a 1 :b "two"})"#);
    let s = result.as_str().unwrap();
    assert!(s.contains(r#""a":1"#) || s.contains(r#""a": 1"#));
    assert!(s.contains(r#""b":"two""#) || s.contains(r#""b": "two""#));
}

#[test]
fn test_json_decode_object() {
    let interp = Interpreter::new();
    let result = interp
        .eval_str(r#"(json/decode "{\"name\": \"alice\", \"age\": 30}")"#)
        .unwrap();
    let name = interp
        .eval_str(r#"(get (json/decode "{\"name\": \"alice\", \"age\": 30}") :name)"#)
        .unwrap();
    assert_eq!(name, Value::string("alice"));
    let age = interp
        .eval_str(r#"(get (json/decode "{\"name\": \"alice\", \"age\": 30}") :age)"#)
        .unwrap();
    assert_eq!(age, Value::int(30));
    assert!(result.as_map_rc().is_some());
}

#[test]
fn test_json_roundtrip_primitives() {
    assert_eq!(eval(r#"(json/decode (json/encode nil))"#), Value::nil());
    assert_eq!(eval(r#"(json/decode (json/encode #t))"#), Value::bool(true));
    assert_eq!(
        eval(r#"(json/decode (json/encode #f))"#),
        Value::bool(false)
    );
    assert_eq!(eval(r#"(json/decode (json/encode 42))"#), Value::int(42));
    assert_eq!(
        eval(r#"(json/decode (json/encode "hello"))"#),
        Value::string("hello")
    );
}

#[test]
fn test_json_pretty_map() {
    let result = eval(r#"(json/encode-pretty {:a 1})"#);
    let s = result.as_str().unwrap();
    assert!(s.contains('\n'));
    assert!(s.contains("  "));
}

// --- KV additional coverage (roundtrip, close+reopen) ---

#[test]
fn test_kv_set_get_roundtrip() {
    let interp = Interpreter::new();
    let tmp = std::env::temp_dir().join("sema-kv-test-rt.json");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    interp
        .eval_str(&format!(r#"(kv/open "rt" "{path}")"#))
        .unwrap();
    interp.eval_str(r#"(kv/set "rt" "x" 42)"#).unwrap();
    interp.eval_str(r#"(kv/set "rt" "y" "hello")"#).unwrap();
    interp.eval_str(r#"(kv/set "rt" "z" #t)"#).unwrap();
    assert_eq!(
        interp.eval_str(r#"(kv/get "rt" "x")"#).unwrap(),
        Value::int(42)
    );
    assert_eq!(
        interp.eval_str(r#"(kv/get "rt" "y")"#).unwrap(),
        Value::string("hello")
    );
    assert_eq!(
        interp.eval_str(r#"(kv/get "rt" "z")"#).unwrap(),
        Value::bool(true)
    );
    interp.eval_str(r#"(kv/close "rt")"#).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_compile_with_macros() {
    let dir = std::env::temp_dir().join("sema_test_compile_macros");
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("macros.sema");
    std::fs::write(
        &src,
        r#"
        (defmacro my-when (test . body)
          `(if ,test (begin ,@body) nil))
        (my-when #t (println "macro-works"))
    "#,
    )
    .unwrap();

    // Compile
    let output = sema_cmd()
        .args(["compile", src.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run the compiled bytecode
    let semac = dir.join("macros.semac");
    let output = sema_cmd().arg(semac.to_str().unwrap()).output().unwrap();
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("macro-works"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_allowed_paths_read_inside() {
    let dir = std::env::temp_dir().join("sema-allowed-paths-test");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.txt");
    std::fs::write(&file, "hello from sandbox").unwrap();

    let sandbox = sema_core::Sandbox::allow_all().with_allowed_paths(vec![dir.clone()]);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(&format!(r#"(file/read "{}")"#, file.display()));
    assert!(
        result.is_ok(),
        "should read inside allowed path: {result:?}"
    );
    assert_eq!(result.unwrap(), Value::string("hello from sandbox"));

    let _ = std::fs::remove_dir_all(&dir);
}

// Regression: sandbox denials must be matched structurally (by SemaError variant)
// rather than by substring-matching the Display message. See OPEN.md
// fragile-error-message-matching.
#[test]
fn test_permission_errors_match_structurally() {
    // Missing capability → PermissionDenied (not PathDenied).
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let err = interp
        .eval_str(r#"(file/exists? "/anything")"#)
        .unwrap_err();
    assert!(matches!(err.inner(), SemaError::PermissionDenied { .. }));
    assert_permission_denied(&err);

    // Path outside allowed dirs → PathDenied, which is also a permission error.
    let dir = std::env::temp_dir().join("sema-perm-structural");
    std::fs::create_dir_all(&dir).unwrap();
    let sandbox = sema_core::Sandbox::allow_all().with_allowed_paths(vec![dir.clone()]);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let err = interp
        .eval_str(r#"(file/exists? "/etc/hosts")"#)
        .unwrap_err();
    assert!(matches!(err.inner(), SemaError::PathDenied { .. }));
    assert_permission_denied(&err);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_allowed_paths_read_outside_denied() {
    let dir = std::env::temp_dir().join("sema-allowed-paths-outside");
    std::fs::create_dir_all(&dir).unwrap();

    let sandbox = sema_core::Sandbox::allow_all().with_allowed_paths(vec![dir.clone()]);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(file/exists? "/etc/hosts")"#);
    assert!(result.is_err(), "should deny access outside allowed path");
    assert_path_denied(&result.unwrap_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_allowed_paths_traversal_denied() {
    let dir = std::env::temp_dir().join("sema-allowed-paths-trav");
    std::fs::create_dir_all(&dir).unwrap();

    let sandbox = sema_core::Sandbox::allow_all().with_allowed_paths(vec![dir.clone()]);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let evil = format!("{}/../../../etc/passwd", dir.display());
    let result = interp.eval_str(&format!(r#"(file/read "{evil}")"#));
    assert!(result.is_err(), "path traversal should be denied");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_allowed_paths_write_inside() {
    let dir = std::env::temp_dir().join("sema-allowed-paths-write");
    std::fs::create_dir_all(&dir).unwrap();

    let sandbox = sema_core::Sandbox::allow_all().with_allowed_paths(vec![dir.clone()]);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let file = dir.join("output.txt");
    let result = interp.eval_str(&format!(r#"(file/write "{}" "written")"#, file.display()));
    assert!(
        result.is_ok(),
        "should write inside allowed path: {result:?}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "written");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_allowed_paths_none_allows_everything() {
    let sandbox = sema_core::Sandbox::allow_all();
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(file/exists? "/tmp")"#);
    assert!(
        result.is_ok(),
        "no allowed_paths should allow all: {result:?}"
    );
}

// ── f-string tests ──

#[test]
fn test_fstring_basic() {
    assert_eq!(
        eval(r#"(let ((name "world")) f"hello ${name}")"#),
        Value::string("hello world")
    );
}

#[test]
fn test_fstring_multiple_interpolations() {
    assert_eq!(
        eval(r#"(let ((a "foo") (b "bar")) f"${a} and ${b}")"#),
        Value::string("foo and bar")
    );
}

#[test]
fn test_fstring_expression() {
    assert_eq!(eval(r#"f"result: ${(+ 1 2)}""#), Value::string("result: 3"));
}

#[test]
fn test_fstring_nested_call() {
    assert_eq!(
        eval(r#"(let ((x 42)) f"the answer is ${x}")"#),
        Value::string("the answer is 42")
    );
}

#[test]
fn test_fstring_no_interpolation() {
    assert_eq!(eval(r#"f"just a string""#), Value::string("just a string"));
}

#[test]
fn test_fstring_keyword_access() {
    assert_eq!(
        eval(r#"(let ((m {:name "Ada"})) f"name: ${(:name m)}")"#),
        Value::string("name: Ada")
    );
}

#[test]
fn test_fstring_escaped_dollar() {
    assert_eq!(eval(r#"f"costs \$5""#), Value::string("costs $5"));
}

#[test]
fn test_fstring_dollar_without_brace() {
    assert_eq!(eval(r#"f"costs $5""#), Value::string("costs $5"));
}

// ── prelude macro tests ──

#[test]
fn test_thread_first() {
    assert_eq!(eval("(-> 5 (+ 3) (* 2))"), Value::int(16));
}

#[test]
fn test_thread_first_bare_fn() {
    assert_eq!(eval(r#"(-> "hello" string-length)"#), Value::int(5));
}

#[test]
fn test_thread_last() {
    assert_eq!(eval("(->> (range 1 6) (filter odd?))"), eval("'(1 3 5)"));
}

#[test]
fn test_thread_last_pipeline() {
    assert_eq!(
        eval("(->> (range 1 6) (map (fn (x) (* x x))) (foldl + 0))"),
        Value::int(55)
    );
}

#[test]
fn test_thread_as() {
    assert_eq!(eval("(as-> 5 x (+ x 3) (* x x) (- x 1))"), Value::int(63));
}

#[test]
fn test_some_thread_non_nil() {
    assert_eq!(
        eval(r#"(some-> {:a {:b 42}} (get :a) (get :b))"#),
        Value::int(42)
    );
}

#[test]
fn test_some_thread_nil() {
    assert_eq!(eval(r#"(some-> {:a nil} (get :a) (get :b))"#), Value::nil());
}

#[test]
fn test_when_let_truthy() {
    assert_eq!(eval(r#"(when-let (x 42) (+ x 1))"#), Value::int(43));
}

#[test]
fn test_when_let_nil() {
    assert_eq!(eval(r#"(when-let (x nil) (+ x 1))"#), Value::nil());
}

#[test]
fn test_if_let_truthy() {
    assert_eq!(eval(r#"(if-let (x 42) (+ x 1) 0)"#), Value::int(43));
}

#[test]
fn test_if_let_nil() {
    assert_eq!(eval(r#"(if-let (x nil) (+ x 1) 0)"#), Value::int(0));
}

// ── nested map functions tests ──

#[test]
fn test_get_in_basic() {
    assert_eq!(
        eval(r#"(get-in {:a {:b {:c 42}}} [:a :b :c])"#),
        Value::int(42)
    );
}

#[test]
fn test_get_in_missing_returns_nil() {
    assert_eq!(eval(r#"(get-in {:a {:b 1}} [:a :c])"#), Value::nil());
}

#[test]
fn test_get_in_missing_with_default() {
    assert_eq!(
        eval(r#"(get-in {:a {:b 1}} [:a :c] "default")"#),
        Value::string("default")
    );
}

#[test]
fn test_get_in_nil_intermediate() {
    assert_eq!(eval(r#"(get-in {:a nil} [:a :b :c])"#), Value::nil());
}

#[test]
fn test_get_in_empty_path() {
    assert_eq!(eval(r#"(get-in {:a 1} [])"#), eval(r#"{:a 1}"#));
}

#[test]
fn test_assoc_in_basic() {
    assert_eq!(
        eval(r#"(get-in (assoc-in {:a {:b 1}} [:a :b] 42) [:a :b])"#),
        Value::int(42)
    );
}

#[test]
fn test_assoc_in_creates_nested() {
    assert_eq!(
        eval(r#"(get-in (assoc-in {} [:a :b :c] 99) [:a :b :c])"#),
        Value::int(99)
    );
}

#[test]
fn test_update_in_basic() {
    assert_eq!(
        eval(r#"(get-in (update-in {:a {:b 10}} [:a :b] (fn (x) (+ x 1))) [:a :b])"#),
        Value::int(11)
    );
}

#[test]
fn test_update_in_missing_key() {
    assert_eq!(
        eval(r#"(get-in (update-in {} [:a :b] (fn (x) (if (nil? x) 1 (+ x 1)))) [:a :b])"#),
        Value::int(1)
    );
}

#[test]
fn test_deep_merge_basic() {
    assert_eq!(
        eval(r#"(get-in (deep-merge {:a {:b 1 :c 2}} {:a {:b 99}}) [:a :c])"#),
        Value::int(2)
    );
    assert_eq!(
        eval(r#"(get-in (deep-merge {:a {:b 1 :c 2}} {:a {:b 99}}) [:a :b])"#),
        Value::int(99)
    );
}

#[test]
fn test_deep_merge_non_map_override() {
    assert_eq!(
        eval(r#"(:a (deep-merge {:a {:b 1}} {:a 42}))"#),
        Value::int(42)
    );
}

#[test]
fn test_deep_merge_multiple() {
    assert_eq!(
        eval(r#"(get-in (deep-merge {:a 1} {:b 2} {:c 3}) [:c])"#),
        Value::int(3)
    );
}

// ── short lambda tests ──

#[test]
fn test_short_lambda_basic() {
    assert_eq!(eval("(map #(+ % 1) '(1 2 3))"), eval("'(2 3 4)"));
}

#[test]
fn test_short_lambda_square() {
    assert_eq!(eval("(map #(* % %) '(1 2 3 4))"), eval("'(1 4 9 16)"));
}

#[test]
fn test_short_lambda_filter() {
    assert_eq!(eval("(filter #(> % 3) '(1 2 3 4 5))"), eval("'(4 5)"));
}

#[test]
fn test_short_lambda_two_args() {
    assert_eq!(eval("(#(+ %1 %2) 3 4)"), Value::int(7));
}

#[test]
fn test_short_lambda_no_args() {
    assert_eq!(eval("(#(+ 1 2))"), Value::int(3));
}

#[test]
fn test_short_lambda_nested_call() {
    assert_eq!(
        eval("(map #(string-length %) '(\"hi\" \"hello\" \"hey\"))"),
        eval("'(2 5 3)")
    );
}

#[test]
fn test_shebang_line_ignored() {
    assert_eq!(eval("#!/usr/bin/env sema\n(+ 1 2)"), Value::int(3));
}

#[test]
fn test_shebang_only() {
    // A file with only a shebang should eval to nil
    assert_eq!(eval("#!/usr/bin/env sema\n"), Value::nil());
}

#[test]
fn test_hash_bang_not_at_start_is_error() {
    // #! not on line 1 col 1 should still error
    let interp = Interpreter::new();
    assert!(interp.eval_str("(+ 1 2)\n#!/usr/bin/env sema").is_err());
}

// ============================================================
// Destructuring in `let` (vector patterns)
// ============================================================

#[test]
fn test_destructure_let_vector() {
    // Basic [a b] destructuring from list
    assert_eq!(eval("(let (([a b] '(1 2))) (+ a b))"), Value::int(3));
}

#[test]
fn test_destructure_let_vector_from_vector() {
    assert_eq!(eval("(let (([a b] [10 20])) (+ a b))"), Value::int(30));
}

#[test]
fn test_destructure_let_vector_rest() {
    // [a b & rest] rest pattern
    assert_eq!(
        eval("(let (([a b & rest] '(1 2 3 4 5))) rest)"),
        eval("'(3 4 5)")
    );
}

#[test]
fn test_destructure_let_vector_rest_empty() {
    assert_eq!(eval("(let (([a b & rest] '(1 2))) rest)"), eval("'()"));
}

#[test]
fn test_destructure_let_wildcard() {
    // _ discards
    assert_eq!(eval("(let (([_ b] '(1 2))) b)"), Value::int(2));
}

#[test]
fn test_destructure_let_nested_vector() {
    assert_eq!(
        eval("(let (([[a b] c] '((1 2) 3))) (+ a b c))"),
        Value::int(6)
    );
}

// ============================================================
// Destructuring in `let` (map patterns)
// ============================================================

#[test]
fn test_destructure_let_map_keys() {
    assert_eq!(
        eval("(let (({:keys [x y]} {:x 10 :y 20})) (+ x y))"),
        Value::int(30)
    );
}

#[test]
fn test_destructure_let_map_missing_key() {
    // Missing key binds to nil
    assert_eq!(eval("(let (({:keys [x y]} {:x 10})) y)"), Value::nil());
}

#[test]
fn test_destructure_let_map_explicit_key() {
    // Explicit key-pattern pair: {:key-name pattern}
    assert_eq!(eval("(let (({:x val} {:x 42})) val)"), Value::int(42));
}

// ============================================================
// Destructuring in `let*`
// ============================================================

#[test]
fn test_destructure_let_star_sequential() {
    assert_eq!(eval("(let* (([a b] '(1 2)) (c (+ a b))) c)"), Value::int(3));
}

// ============================================================
// Destructuring in `define`
// ============================================================

#[test]
fn test_destructure_define_vector() {
    assert_eq!(
        eval("(begin (define [a b c] '(1 2 3)) (+ a b c))"),
        Value::int(6)
    );
}

#[test]
fn test_destructure_define_map() {
    assert_eq!(
        eval("(begin (define {:keys [name age]} {:name \"Alice\" :age 30}) age)"),
        Value::int(30)
    );
}

// ============================================================
// Destructuring in `lambda` parameters
// ============================================================

#[test]
fn test_destructure_lambda_vector_param() {
    assert_eq!(eval("((lambda ([a b]) (+ a b)) '(1 2))"), Value::int(3));
}

#[test]
fn test_destructure_lambda_map_param() {
    assert_eq!(
        eval("((lambda ({:keys [x y]}) (+ x y)) {:x 3 :y 4})"),
        Value::int(7)
    );
}

#[test]
fn test_destructure_lambda_mixed_params() {
    assert_eq!(
        eval("((lambda (a [b c]) (+ a b c)) 10 '(20 30))"),
        Value::int(60)
    );
}

#[test]
fn test_destructure_define_function_with_destructuring() {
    assert_eq!(
        eval("(begin (define sum-pair (lambda ([a b]) (+ a b))) (sum-pair '(3 4)))"),
        Value::int(7)
    );
}

// ============================================================
// Pattern matching `match`
// ============================================================

#[test]
fn test_match_literal_int() {
    assert_eq!(
        eval("(match 42 (42 \"found\") (_ \"nope\"))"),
        Value::string("found")
    );
}

#[test]
fn test_match_literal_string() {
    assert_eq!(
        eval(r#"(match "hello" ("hello" 1) ("world" 2) (_ 0))"#),
        Value::int(1)
    );
}

#[test]
fn test_match_literal_keyword() {
    assert_eq!(
        eval("(match :ok (:ok \"success\") (:err \"failure\"))"),
        Value::string("success")
    );
}

#[test]
fn test_match_literal_bool() {
    assert_eq!(
        eval("(match #t (#t \"yes\") (#f \"no\"))"),
        Value::string("yes")
    );
}

#[test]
fn test_match_wildcard() {
    assert_eq!(
        eval("(match 99 (1 \"one\") (2 \"two\") (_ \"other\"))"),
        Value::string("other")
    );
}

#[test]
fn test_match_symbol_binding() {
    assert_eq!(eval("(match 42 (x (+ x 8)))"), Value::int(50));
}

#[test]
fn test_match_vector_pattern() {
    assert_eq!(eval("(match '(1 2 3) ([a b c] (+ a b c)))"), Value::int(6));
}

#[test]
fn test_match_vector_rest() {
    assert_eq!(
        eval("(match '(1 2 3 4) ([a & rest] rest))"),
        eval("'(2 3 4)")
    );
}

#[test]
fn test_match_map_keys() {
    assert_eq!(
        eval("(match {:x 10 :y 20} ({:keys [x y]} (+ x y)))"),
        Value::int(30)
    );
}

#[test]
fn test_match_guard() {
    assert_eq!(
        eval("(match 5 (x when (> x 10) \"big\") (x when (> x 0) \"small\") (_ \"zero\"))"),
        Value::string("small")
    );
}

#[test]
fn test_match_guard_with_binding() {
    assert_eq!(
        eval("(match 15 (x when (> x 10) (+ x 1)) (x x))"),
        Value::int(16)
    );
}

#[test]
fn test_match_no_match_raises() {
    // Strict `match` raises when no clause matches (D3), and the error carries
    // the unmatched value (via __vm-match-failed).
    let err = eval_err("(match 42 (1 \"one\") (2 \"two\"))").to_string();
    assert!(
        err.contains("no clause matched"),
        "expected match-failed error, got: {err}"
    );
    assert!(
        err.contains("42"),
        "error should carry the unmatched value, got: {err}"
    );
}

#[test]
fn test_match_star_no_match_returns_nil() {
    // The lenient `match*` returns nil on no-match.
    assert_eq!(eval("(match* 42 (1 \"one\") (2 \"two\"))"), Value::nil());
}

#[test]
fn test_match_multiple_body_exprs() {
    assert_eq!(eval("(match 1 (1 (define x 10) (+ x 5)))"), Value::int(15));
}

#[test]
fn test_match_nested_patterns() {
    assert_eq!(
        eval("(match '(1 (2 3)) ([a [b c]] (+ a b c)))"),
        Value::int(6)
    );
}

#[test]
fn test_match_map_structural() {
    // Structural map match: key must exist in value
    assert_eq!(
        eval("(match {:type :ok :val 42} ({:type :ok :val v} v) (_ nil))"),
        Value::int(42)
    );
}

#[test]
fn test_match_map_structural_no_match() {
    assert_eq!(
        eval("(match {:type :err} ({:type :ok :val v} v) (_ \"fallback\"))"),
        Value::string("fallback")
    );
}

// ============================================================
// Edge cases: destructuring errors
// ============================================================

#[test]
fn test_destructure_too_few_elements() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(let (([a b c] '(1 2))) a)").is_err());
}

#[test]
fn test_destructure_too_many_elements() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(let (([a b] '(1 2 3))) a)").is_err());
}

#[test]
fn test_destructure_rest_too_few() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(let (([a b & rest] '(1))) a)").is_err());
}

#[test]
fn test_destructure_non_list_value() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(let (([a b] 42)) a)").is_err());
}

#[test]
fn test_destructure_map_non_map_value() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(let (({:keys [x]} '(1 2))) x)").is_err());
}

#[test]
fn test_destructure_amp_no_rest_pattern() {
    let interp = Interpreter::new();
    assert!(interp.eval_str("(let (([a &] '(1 2))) a)").is_err());
}

#[test]
fn test_destructure_wildcard_in_vector() {
    // Multiple wildcards should work
    assert_eq!(eval("(let (([_ _ c] '(1 2 3))) c)"), Value::int(3));
}

#[test]
fn test_destructure_map_keys_as_list() {
    // {:keys (x y)} with list syntax instead of vector
    assert_eq!(
        eval("(let (({:keys (x y)} {:x 5 :y 6})) (+ x y))"),
        Value::int(11)
    );
}

#[test]
fn test_destructure_hashmap() {
    // Destructuring should work with hashmaps too
    assert_eq!(
        eval("(let (({:keys [x]} (hash-map :x 99))) x)"),
        Value::int(99)
    );
}

// ============================================================
// Edge cases: match patterns
// ============================================================

#[test]
fn test_match_vector_pattern_wrong_type() {
    // Vector pattern against non-sequence falls through
    assert_eq!(
        eval("(match 42 ([a b] \"list\") (_ \"other\"))"),
        Value::string("other")
    );
}

#[test]
fn test_match_vector_length_mismatch() {
    assert_eq!(
        eval("(match '(1 2 3) ([a b] \"two\") (_ \"other\"))"),
        Value::string("other")
    );
}

#[test]
fn test_match_map_non_map_falls_through() {
    assert_eq!(
        eval("(match 42 ({:keys [x]} \"map\") (_ \"other\"))"),
        Value::string("other")
    );
}

#[test]
fn test_match_map_missing_key_falls_through() {
    // Structural match: required key missing → no match
    assert_eq!(
        eval("(match {:a 1} ({:b val} \"found\") (_ \"nope\"))"),
        Value::string("nope")
    );
}

#[test]
fn test_match_quoted_literal() {
    assert_eq!(
        eval("(match 'foo ('foo \"yes\") (_ \"no\"))"),
        Value::string("yes")
    );
}

#[test]
fn test_match_quoted_literal_no_match() {
    assert_eq!(
        eval("(match 'bar ('foo \"yes\") (_ \"no\"))"),
        Value::string("no")
    );
}

#[test]
fn test_match_nil() {
    assert_eq!(
        eval("(match nil (nil \"null\") (_ \"other\"))"),
        Value::string("null")
    );
}

#[test]
fn test_match_vector_element_mismatch() {
    // First element matches but second doesn't
    assert_eq!(
        eval("(match '(1 2) ([1 3] \"a\") ([1 2] \"b\"))"),
        Value::string("b")
    );
}

#[test]
fn test_match_rest_pattern() {
    // Rest pattern in match with empty rest
    assert_eq!(eval("(match '(1) ([a & rest] rest))"), eval("'()"));
}

#[test]
fn test_match_clause_as_list() {
    // Clauses can be lists too, not just vectors
    assert_eq!(eval("(match 42 (42 \"yes\"))"), Value::string("yes"));
}

#[test]
fn test_match_deeply_nested() {
    assert_eq!(
        eval("(match '(1 (2 (3))) ([a [b [c]]] (+ a b c)))"),
        Value::int(6)
    );
}

#[test]
fn test_match_guard_all_fail() {
    // All guards fail → no clause matches. `match*` is lenient (nil); strict
    // `match` raises (covered by test_match_no_match_raises).
    assert_eq!(
        eval("(match* 5 (x when (> x 100) \"big\") (x when (< x 0) \"neg\"))"),
        Value::nil()
    );
}

#[test]
fn test_match_hashmap() {
    // match should work with hashmaps
    assert_eq!(
        eval("(match (hash-map :x 42) ({:keys [x]} x))"),
        Value::int(42)
    );
}

// ============================================================
// Edge cases: lambda destructuring
// ============================================================

#[test]
fn test_destructure_lambda_with_rest() {
    // Lambda with destructuring + dot rest
    assert_eq!(
        eval("((lambda ([a b] . rest) (list a b rest)) '(1 2) 3 4)"),
        eval("'(1 2 (3 4))")
    );
}

#[test]
fn test_destructure_define_vector_in_let_star() {
    // let* with destructuring sees previous bindings
    assert_eq!(
        eval("(let* ((data '(10 20)) ([a b] data)) (+ a b))"),
        Value::int(30)
    );
}

#[test]
fn test_destructure_nested_map_in_vector() {
    // [a {:keys [b]}] nested pattern
    assert_eq!(
        eval("(let (([a {:keys [b]}] (list 1 {:b 2}))) (+ a b))"),
        Value::int(3)
    );
}

// ── http response helpers ──────────────────────────────────────

#[test]
fn test_http_ok_with_string() {
    let result = eval(r#"(http/ok "hello")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
    assert_eq!(
        map.get(&Value::keyword("body")),
        Some(&Value::string("\"hello\""))
    );
}

#[test]
fn test_http_ok_with_map() {
    let result = eval(r#"(http/ok {:msg "hi"})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
    let body = map.get(&Value::keyword("body")).unwrap();
    assert!(body.as_str().unwrap().contains("msg"));
}

#[test]
fn test_http_not_found() {
    let result = eval(r#"(http/not-found "gone")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(404)));
}

#[test]
fn test_http_redirect() {
    let result = eval(r#"(http/redirect "/login")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(302)));
    let headers = map
        .get(&Value::keyword("headers"))
        .unwrap()
        .as_map_rc()
        .unwrap();
    assert_eq!(
        headers.get(&Value::string("location")),
        Some(&Value::string("/login"))
    );
}

#[test]
fn test_http_error_custom_status() {
    let result = eval(r#"(http/error 422 {:errors ["invalid"]})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(422)));
}

#[test]
fn test_http_html() {
    let result = eval(r#"(http/html "<h1>Hi</h1>")"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
    let headers = map
        .get(&Value::keyword("headers"))
        .unwrap()
        .as_map_rc()
        .unwrap();
    assert_eq!(
        headers.get(&Value::string("content-type")),
        Some(&Value::string("text/html"))
    );
    assert_eq!(
        map.get(&Value::keyword("body")),
        Some(&Value::string("<h1>Hi</h1>"))
    );
}

#[test]
fn test_http_text() {
    let result = eval(r#"(http/text "plain text")"#);
    let map = result.as_map_rc().unwrap();
    let headers = map
        .get(&Value::keyword("headers"))
        .unwrap()
        .as_map_rc()
        .unwrap();
    assert_eq!(
        headers.get(&Value::string("content-type")),
        Some(&Value::string("text/plain"))
    );
}

#[test]
fn test_http_created() {
    let result = eval(r#"(http/created {:id 1})"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(201)));
}

#[test]
fn test_http_no_content() {
    let result = eval(r#"(http/no-content)"#);
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(204)));
    assert_eq!(map.get(&Value::keyword("body")), Some(&Value::string("")));
}

#[test]
fn test_http_router_basic() {
    let result = eval(
        r#"
        (let ((router (http/router (list [:get "/" (fn (req) (http/ok "home"))]))))
          (router {:method :get :path "/" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
}

#[test]
fn test_http_router_params() {
    let result = eval(
        r#"
        (let ((router (http/router (list [:get "/users/:id" (fn (req) (http/ok (:params req)))]))))
          (router {:method :get :path "/users/42" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = result.as_map_rc().unwrap();
    let body = map.get(&Value::keyword("body")).unwrap();
    assert!(body.as_str().unwrap().contains("42"));
}

#[test]
fn test_http_router_404() {
    let result = eval(
        r#"
        (let ((router (http/router (list [:get "/" (fn (req) (http/ok "home"))]))))
          (router {:method :get :path "/missing" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(404)));
}

#[test]
fn test_http_router_method_matching() {
    let result = eval(
        r#"
        (let ((router (http/router (list [:post "/data" (fn (req) (http/ok "posted"))]))))
          (router {:method :get :path "/data" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(404)));
}

#[test]
fn test_http_router_any_method() {
    let result = eval(
        r#"
        (let ((router (http/router (list [:any "/health" (fn (req) (http/ok "up"))]))))
          (router {:method :delete :path "/health" :headers {} :query {} :params {} :body "" :remote "127.0.0.1"}))
    "#,
    );
    let map = result.as_map_rc().unwrap();
    assert_eq!(map.get(&Value::keyword("status")), Some(&Value::int(200)));
}

#[test]
#[ignore] // requires network
fn test_http_serve_basic() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(r#"(http/serve (fn (req) (http/ok {:path (:path req)})) {:port 19876})"#)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    // Wait for server to start
    std::thread::sleep(Duration::from_millis(1500));

    // Make request using reqwest
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19876/test")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().expect("Failed to parse JSON");
    assert_eq!(body["path"], "/test");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_with_router() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:get "/hello/:name" (fn (req)
                   (http/ok {:greeting (string-append "hi " (:name (:params req)))}))]
                 [:get "/health" (fn (_) (http/ok {:status "up"}))]])
              {:port 19877})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();

    // Test parameterized route
    let resp = client
        .get("http://127.0.0.1:19877/hello/Ada")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["greeting"], "hi Ada");

    // Test health route
    let resp = client
        .get("http://127.0.0.1:19877/health")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET /health");
    assert_eq!(resp.status(), 200);

    // Test 404
    let resp = client
        .get("http://127.0.0.1:19877/nonexistent")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET /nonexistent");
    assert_eq!(resp.status(), 404);

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_sse() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:get "/stream"
                  (fn (req)
                    (http/stream (fn (send)
                      (send "hello")
                      (send "world"))))]])
              {:port 19878})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let client = reqwest::blocking::Client::new();
    let resp = client
        .get("http://127.0.0.1:19878/stream")
        .timeout(Duration::from_secs(5))
        .send()
        .expect("Failed to GET /stream");

    let body = resp.text().unwrap();
    assert!(body.contains("hello"), "body should contain hello: {body}");
    assert!(body.contains("world"), "body should contain world: {body}");

    child.kill().ok();
    child.wait().ok();
}

#[test]
#[ignore] // requires network
fn test_http_serve_websocket() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut child = Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("-e")
        .arg(
            r#"
            (http/serve
              (http/router
                [[:ws "/echo" (fn (conn)
                  (let ((msg ((:recv conn))))
                    (when msg
                      ((:send conn) (string-append "echo:" msg)))))]])
              {:port 19879})
        "#,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn sema");

    std::thread::sleep(Duration::from_millis(1500));

    let (mut ws, _) = tungstenite::connect("ws://127.0.0.1:19879/echo").expect("WS connect failed");
    ws.send(tungstenite::Message::Text("hello".into())).unwrap();
    let reply = ws.read().unwrap();
    assert_eq!(reply.into_text().unwrap(), "echo:hello");

    child.kill().ok();
    child.wait().ok();
}

// ===========================================================================
// sema build — standalone executable tests
// ===========================================================================

/// Create a unique temporary directory for a build test.
fn build_test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sema-build-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_sema_build_basic() {
    let dir = build_test_dir("basic");

    std::fs::write(
        dir.join("hello.sema"),
        r#"(println "hello from bundled sema")"#,
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("hello.sema").to_str().unwrap(),
            "-o",
            dir.join("hello").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.join("hello").exists(), "output executable not created");

    // Run the bundled executable
    let run = std::process::Command::new(dir.join("hello"))
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "hello from bundled sema"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_sema_build_with_imports() {
    let dir = build_test_dir("imports");
    std::fs::create_dir_all(dir.join("lib")).unwrap();

    // Library module
    std::fs::write(
        dir.join("lib/math.sema"),
        "(module math (export square) (define (square x) (* x x)))",
    )
    .unwrap();

    // Main file that imports it
    std::fs::write(
        dir.join("app.sema"),
        r#"(import "lib/math.sema") (println (square 7))"#,
    )
    .unwrap();

    // Build
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("app.sema").to_str().unwrap(),
            "-o",
            dir.join("app").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Remove source files to prove the VFS is working
    std::fs::remove_dir_all(dir.join("lib")).unwrap();
    std::fs::remove_file(dir.join("app.sema")).unwrap();

    // Run
    let run = std::process::Command::new(dir.join("app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "49");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `sema build` must embed + resolve imports written with weird path spellings
/// (`./x/../x`, climbing subdirs then back to root) AND unicode module names —
/// once the source is gone, only the embedded archive can satisfy them.
#[test]
fn test_sema_build_weird_and_unicode_imports_embedded() {
    let dir = build_test_dir("weird-unicode");
    std::fs::create_dir_all(dir.join("a/b/c")).unwrap();
    std::fs::create_dir_all(dir.join("lïb-café")).unwrap();

    // A deep chain where every hop uses a weird relative spelling.
    std::fs::write(
        dir.join("top.sema"),
        "(module top (export tv) (define tv 100))",
    )
    .unwrap();
    std::fs::write(
        dir.join("a/b/c/m3.sema"),
        r#"(module m3 (export tv) (import "../../../top.sema"))"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("a/b/m2.sema"),
        r#"(module m2 (export tv) (import "./c/../c/m3.sema"))"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("a/m1.sema"),
        r#"(module m1 (export tv) (import "../a/./b/m2.sema"))"#,
    )
    .unwrap();
    // A unicode module dir + a weird spelling of it.
    std::fs::write(
        dir.join("lïb-café/µtil.sema"),
        "(module u (export uv) (define uv 42))",
    )
    .unwrap();
    std::fs::write(
        dir.join("app.sema"),
        "(import \"././a/m1.sema\")\n\
         (import \"./lïb-café/../lïb-café/µtil.sema\")\n\
         (println (+ tv uv))\n",
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("app.sema").to_str().unwrap(),
            "-o",
            dir.join("app").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");
    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Remove every source so resolution can only come from the embedded archive.
    std::fs::remove_dir_all(dir.join("a")).unwrap();
    std::fs::remove_dir_all(dir.join("lïb-café")).unwrap();
    std::fs::remove_file(dir.join("top.sema")).unwrap();
    std::fs::remove_file(dir.join("app.sema")).unwrap();

    let run = std::process::Command::new(dir.join("app"))
        .output()
        .expect("failed to run bundled executable");
    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "142");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `sema compile` produces a `.semac` whose imports resolve from the filesystem
/// at runtime. Weird spellings + unicode paths must resolve there too.
#[test]
fn test_compile_multifile_imports_resolve_from_fs() {
    let dir = build_test_dir("compile-mf");
    std::fs::create_dir_all(dir.join("lïb")).unwrap();
    std::fs::write(
        dir.join("lïb/µtil.sema"),
        "(module u (export answer) (define answer 42))",
    )
    .unwrap();
    std::fs::write(
        dir.join("app.sema"),
        "(import \"./lïb/../lïb/µtil.sema\")\n(println answer)\n",
    )
    .unwrap();

    let compiled = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["compile", dir.join("app.sema").to_str().unwrap()])
        .output()
        .expect("failed to run sema compile");
    assert!(
        compiled.status.success(),
        "sema compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    // Run the .semac from its directory; the (still-present) source imports
    // resolve from the filesystem.
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .current_dir(&dir)
        .arg("app.semac")
        .output()
        .expect("failed to run .semac");
    assert!(
        run.status.success(),
        "running .semac failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_sema_build_with_include() {
    let dir = build_test_dir("include");
    std::fs::create_dir_all(dir.join("data")).unwrap();

    // Data file to include
    std::fs::write(dir.join("data/config.json"), r#"{"name": "test"}"#).unwrap();

    // Main file reads the included asset
    std::fs::write(
        dir.join("app.sema"),
        r#"(println (file/read "data/config.json"))"#,
    )
    .unwrap();

    // Build with --include
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("app.sema").to_str().unwrap(),
            "--include",
            dir.join("data").to_str().unwrap(),
            "-o",
            dir.join("app").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Remove source + data to prove VFS works
    std::fs::remove_dir_all(dir.join("data")).unwrap();
    std::fs::remove_file(dir.join("app.sema")).unwrap();

    // Run
    let run = std::process::Command::new(dir.join("app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        r#"{"name": "test"}"#
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_sema_build_passes_args() {
    let dir = build_test_dir("args");

    std::fs::write(dir.join("args.sema"), r#"(println (length (sys/args)))"#).unwrap();

    // Build
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("args.sema").to_str().unwrap(),
            "-o",
            dir.join("args").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Run with extra arguments
    let run = std::process::Command::new(dir.join("args"))
        .args(["--foo", "bar"])
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    // argv should be: ["/path/to/args", "--foo", "bar"] = 3
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "3");

    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// VFS interception tests — exercise file/read, file/exists?, import, load
// with a pre-initialized VFS (no subprocess needed).
//
// VFS is process-global (RwLock), so all VFS-mutating in-process tests are
// consolidated into a single test function to avoid races from cargo's
// parallel test runner. Each sub-section creates a fresh Interpreter.
// ===========================================================================

#[test]
fn test_vfs_in_process() {
    // Initialize VFS once with ALL files needed by every sub-test
    let mut files = std::collections::HashMap::new();

    // interception: file/read, file/read-bytes, file/exists?, file/read-lines
    files.insert("hello.txt".to_string(), b"hello from VFS".to_vec());
    files.insert("bad.bin".to_string(), vec![0xFF, 0xFE]);
    files.insert("data.bin".to_string(), vec![1u8, 2, 3]);
    files.insert("exists.txt".to_string(), b"data".to_vec());
    files.insert("lines.txt".to_string(), b"alpha\nbeta\ngamma".to_vec());
    files.insert(
        "lib/math.sema".to_string(),
        b"(module math (export square) (define (square x) (* x x)))".to_vec(),
    );
    files.insert(
        "counter.sema".to_string(),
        b"(module counter (export n) (define n 42))".to_vec(),
    );
    files.insert(
        "load-defs.sema".to_string(),
        b"(define loaded-val 123)".to_vec(),
    );
    files.insert("bad.sema".to_string(), vec![0xFF, 0xFE]);

    // path resolution
    files.insert("lib/a.sema".to_string(), b"(define a-val 1)".to_vec());
    files.insert(
        "main.sema".to_string(),
        b"(load \"lib/a.sema\") a-val".to_vec(),
    );
    files.insert("base.sema".to_string(), b"(define base 100)".to_vec());
    files.insert(
        "mid.sema".to_string(),
        b"(load \"base.sema\") (define mid (+ base 1))".to_vec(),
    );
    files.insert(
        "lib/utils.sema".to_string(),
        b"(module utils (export double) (define (double x) (* x 2)))".to_vec(),
    );
    files.insert(
        "fmt-helpers.sema".to_string(),
        b"(define (fmt x) (format \"v=~a\" x))".to_vec(),
    );
    files.insert(
        "lib/package.sema".to_string(),
        b"(module mymod (export val) (define val 99))".to_vec(),
    );
    files.insert("data/config.json".to_string(), b"{}".to_vec());
    files.insert("assets/greeting.txt".to_string(), b"howdy".to_vec());
    files.insert("exists.sema".to_string(), b"(define x 1)".to_vec());
    files.insert("config.txt".to_string(), b"agent-config".to_vec());
    files.insert(
        "multi.sema".to_string(),
        b"(module multi (export a b c) (define a 1) (define b 2) (define c 3))".to_vec(),
    );

    // package imports
    files.insert(
        "github.com/test/vfslib".to_string(),
        b"(module vfslib (export vfs-val) (define vfs-val 777))".to_vec(),
    );
    files.insert(
        "local-lib.sema".to_string(),
        b"(module local (export local-val) (define local-val 1))".to_vec(),
    );
    files.insert(
        "github.com/test/remote".to_string(),
        b"(module remote (export remote-val) (define remote-val 200))".to_vec(),
    );
    files.insert(
        "github.com/test/translib".to_string(),
        b"(import \"helpers.sema\") (define main-val (+ helper-val 1))".to_vec(),
    );
    files.insert(
        "github.com/test/translib/helpers.sema".to_string(),
        b"(define helper-val 42)".to_vec(),
    );
    files.insert(
        "json-utils".to_string(),
        b"(import \"helpers.sema\") (module json-utils (export json-val) (define json-val (+ helper-val 1)))".to_vec(),
    );
    files.insert(
        "json-utils/helpers.sema".to_string(),
        b"(define helper-val 99)".to_vec(),
    );
    files.insert(
        "github.com/a/lib".to_string(),
        b"(import \"helpers.sema\") (module alib (export a-val) (define a-val helper-val))"
            .to_vec(),
    );
    files.insert(
        "github.com/a/lib/helpers.sema".to_string(),
        b"(define helper-val 100)".to_vec(),
    );
    files.insert(
        "github.com/b/util".to_string(),
        b"(module butil (export b-val) (define b-val 5))".to_vec(),
    );
    files.insert(
        "json-tools".to_string(),
        b"(import \"github.com/x/parser\") (module json-tools (export tool-val) (define tool-val (+ parser-val 1)))".to_vec(),
    );
    files.insert(
        "github.com/x/parser".to_string(),
        b"(module xparser (export parser-val) (define parser-val 50))".to_vec(),
    );
    files.insert(
        "github.com/c/shared".to_string(),
        b"(module cshared (export shared-val) (define shared-val 5))".to_vec(),
    );
    files.insert(
        "github.com/b/lib".to_string(),
        b"(import \"helpers.sema\") (module blib (export b-val) (define b-val helper-val))"
            .to_vec(),
    );
    files.insert(
        "github.com/b/lib/helpers.sema".to_string(),
        b"(define helper-val 200)".to_vec(),
    );
    files.insert(
        "github.com/x/deeplib".to_string(),
        b"(import \"src/utils.sema\") (module deeplib (export deep-val) (define deep-val (+ util-val 1)))".to_vec(),
    );
    files.insert(
        "github.com/x/deeplib/src/utils.sema".to_string(),
        b"(define util-val 77)".to_vec(),
    );
    files.insert(
        "github.com/l1/pkg".to_string(),
        b"(import \"github.com/l2/pkg\") (module l1 (export l1-val) (define l1-val (+ l2-val 100)))".to_vec(),
    );
    files.insert(
        "github.com/l2/pkg".to_string(),
        b"(import \"github.com/l3/pkg\") (module l2 (export l2-val) (define l2-val (+ l3-val 10)))"
            .to_vec(),
    );
    files.insert(
        "github.com/l3/pkg".to_string(),
        b"(module l3 (export l3-val) (define l3-val 1))".to_vec(),
    );
    files.insert(
        "github.com/x/loadpkg".to_string(),
        b"(load \"defs.sema\") (module loadpkg (export result) (define result (+ loaded-val 1)))"
            .to_vec(),
    );
    files.insert(
        "github.com/x/loadpkg/defs.sema".to_string(),
        b"(define loaded-val 10)".to_vec(),
    );
    files.insert(
        "github.com/x/nestload".to_string(),
        b"(load \"init.sema\") (module nestload (export result) (define result (+ nested-val 1)))"
            .to_vec(),
    );
    files.insert(
        "github.com/x/nestload/init.sema".to_string(),
        b"(import \"constants.sema\") (define nested-val (+ const-val 10))".to_vec(),
    );
    files.insert(
        "github.com/x/nestload/constants.sema".to_string(),
        b"(define const-val 5)".to_vec(),
    );
    files.insert(
        "github.com/x/lib".to_string(),
        b"(import \"src/utils.sema\") (module xlib (export deep-result git-val) (define deep-result (+ util-val 1)) (define git-val 3))".to_vec(),
    );
    files.insert(
        "github.com/x/lib/src/utils.sema".to_string(),
        b"(import \"common.sema\") (define util-val (+ common-val 10))".to_vec(),
    );
    files.insert(
        "github.com/x/lib/src/common.sema".to_string(),
        b"(define common-val 5)".to_vec(),
    );

    sema_core::vfs::init_vfs(files);

    // ===== Interception tests =====

    // --- file/read ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(file/read "hello.txt")"#).unwrap();
        assert_eq!(result, Value::string("hello from VFS"));
    }

    // --- file/read invalid UTF-8 ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(file/read "bad.bin")"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("UTF-8"));
    }

    // --- file/read-bytes ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(file/read-bytes "data.bin")"#).unwrap();
        let bv = result.as_bytevector().unwrap();
        assert_eq!(bv, &[1u8, 2, 3]);
    }

    // --- file/exists? ---
    {
        let interp = Interpreter::new();
        assert_eq!(
            interp.eval_str(r#"(file/exists? "exists.txt")"#).unwrap(),
            Value::bool(true)
        );
        assert_eq!(
            interp.eval_str(r#"(file/exists? "ghost.txt")"#).unwrap(),
            Value::bool(false)
        );
    }

    // --- file/read-lines ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(file/read-lines "lines.txt")"#).unwrap();
        let items = result.as_list().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::string("alpha"));
        assert_eq!(items[1], Value::string("beta"));
        assert_eq!(items[2], Value::string("gamma"));
    }

    // --- import ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "lib/math.sema") (square 7))"#)
            .unwrap();
        assert_eq!(result, Value::int(49));
    }

    // --- import cached ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "counter.sema") (import "counter.sema") n)"#)
            .unwrap();
        assert_eq!(result, Value::int(42));
    }

    // --- load ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (load "load-defs.sema") loaded-val)"#)
            .unwrap();
        assert_eq!(result, Value::int(123));
    }

    // --- import invalid UTF-8 ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(import "bad.sema")"#);
        assert!(result.is_err());
    }

    // --- load invalid UTF-8 ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(load "bad.sema")"#);
        assert!(result.is_err());
    }

    // ===== Path resolution tests =====

    // --- load from subdirectory ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (load "lib/a.sema") a-val)"#)
            .unwrap();
        assert_eq!(result, Value::int(1));
    }

    // --- chained loads ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(begin (load "mid.sema") mid)"#).unwrap();
        assert_eq!(result, Value::int(101));
    }

    // --- import from subdirectory ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "lib/utils.sema") (double 21))"#)
            .unwrap();
        assert_eq!(result, Value::int(42));
    }

    // --- load then import ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (load "fmt-helpers.sema") (import "lib/package.sema") (fmt val))"#)
            .unwrap();
        assert_eq!(result, Value::string("v=99"));
    }

    // --- file/exists? subdirectory ---
    {
        let interp = Interpreter::new();
        assert_eq!(
            interp
                .eval_str(r#"(file/exists? "data/config.json")"#)
                .unwrap(),
            Value::bool(true)
        );
        assert_eq!(
            interp
                .eval_str(r#"(file/exists? "data/missing.json")"#)
                .unwrap(),
            Value::bool(false)
        );
    }

    // --- file/read subdirectory ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(file/read "assets/greeting.txt")"#)
            .unwrap();
        assert_eq!(result, Value::string("howdy"));
    }

    // --- load missing file errors ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(r#"(load "nonexistent.sema")"#);
        assert!(result.is_err(), "loading missing VFS file should error");
    }

    // --- VFS accessible after LLM call ---
    {
        let interp = Interpreter::new();
        let before = interp.eval_str(r#"(file/read "config.txt")"#).unwrap();
        assert_eq!(before, Value::string("agent-config"));

        let llm_result = interp.eval_str(
            r#"(let ((provider (llm/auto-configure)))
                 (if (nil? provider)
                   "no-provider"
                   (llm/ask provider "reply with exactly: OK")))"#,
        );
        let _ = llm_result;

        let after = interp.eval_str(r#"(file/read "config.txt")"#).unwrap();
        assert_eq!(after, Value::string("agent-config"));
        assert!(sema_core::vfs::is_vfs_active());
    }

    // --- import selective ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "multi.sema" a c) (+ a c))"#)
            .unwrap();
        assert_eq!(result, Value::int(4));
    }

    // ===== Package import tests =====

    // --- package imports via VFS ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "github.com/test/vfslib") vfs-val)"#)
            .unwrap();
        assert_eq!(result, Value::int(777), "VFS package import should work");
    }

    // --- mixed local and package ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(
                r#"(begin
                    (import "local-lib.sema")
                    (import "github.com/test/remote")
                    (+ local-val remote-val))"#,
            )
            .unwrap();
        assert_eq!(result, Value::int(201), "mixed VFS imports should work");
    }

    // --- transitive package imports ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(
                r#"(begin
                    (import "github.com/test/translib" main-val)
                    main-val)"#,
            )
            .unwrap();
        assert_eq!(
            result,
            Value::int(43),
            "transitive package imports should resolve via VFS"
        );
    }

    // --- registry short-name package with transitive deps ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "json-utils" json-val) json-val)"#)
            .unwrap();
        assert_eq!(result, Value::int(100));
    }

    // --- registry imports git package ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "json-tools" tool-val) tool-val)"#)
            .unwrap();
        assert_eq!(result, Value::int(51));
    }

    // --- package subdirectory imports ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "github.com/x/deeplib" deep-val) deep-val)"#)
            .unwrap();
        assert_eq!(result, Value::int(78));
    }

    // --- deep package chain ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "github.com/l1/pkg" l1-val) l1-val)"#)
            .unwrap();
        assert_eq!(result, Value::int(111));
    }

    // --- load within package ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "github.com/x/loadpkg" result) result)"#)
            .unwrap();
        assert_eq!(result, Value::int(11));
    }

    // --- load with nested import within package ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(r#"(begin (import "github.com/x/nestload" result) result)"#)
            .unwrap();
        assert_eq!(result, Value::int(16));
    }

    // --- mixed local, registry, and git ---
    {
        let interp = Interpreter::new();
        let result = interp
            .eval_str(
                r#"(begin
                    (import "local-lib.sema")
                    (import "json-utils")
                    (import "github.com/x/lib")
                    (+ local-val json-val git-val))"#,
            )
            .unwrap();
        assert_eq!(result, Value::int(104));
    }

    // --- deep subdir chain within package ---
    {
        let interp = Interpreter::new();
        let result =
            interp.eval_str(r#"(begin (import "github.com/x/lib" deep-result) deep-result)"#);
        assert!(
            result.is_ok(),
            "deep subdir chain should work: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap(), Value::int(16));
    }

    // --- package cache collision ---
    {
        let interp = Interpreter::new();
        let result = interp.eval_str(
            r#"(begin
                (import "github.com/a/lib" a-val)
                (import "github.com/b/lib" b-val)
                (list a-val b-val))"#,
        );
        assert!(result.is_ok(), "should not error: {:?}", result.err());
        let r = result.unwrap();
        let items = r.as_list().unwrap();
        assert_eq!(items[0], Value::int(100), "a-val should be 100");
        assert_eq!(
            items[1],
            Value::int(200),
            "b-val should be 200, not 100 from cache"
        );
    }
}

#[test]
fn test_sys_sema_home() {
    let result = eval("(sys/sema-home)");
    assert!(result.is_string(), "sys/sema-home should return a string");
    let s = result.as_str().unwrap();
    assert!(
        s.contains(".sema") || std::env::var("SEMA_HOME").is_ok(),
        "expected .sema in path: {s}"
    );
}

// ===========================================================================
// sema build — multi-file load (the pi-sema pattern)
// ===========================================================================

#[test]
fn test_sema_build_with_load() {
    let dir = build_test_dir("load");

    // Helper module loaded by main
    std::fs::write(
        dir.join("util.sema"),
        r#"(define (greet name) (format "hello ~a" name))"#,
    )
    .unwrap();

    // Main file uses load (not import)
    std::fs::write(
        dir.join("main.sema"),
        r#"(load "util.sema") (println (greet "world"))"#,
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("main.sema").to_str().unwrap(),
            "-o",
            dir.join("app").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Remove source files to prove VFS is working
    std::fs::remove_file(dir.join("util.sema")).unwrap();
    std::fs::remove_file(dir.join("main.sema")).unwrap();

    let run = std::process::Command::new(dir.join("app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "hello world");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_sema_build_with_chained_loads() {
    let dir = build_test_dir("chained-loads");

    // c.sema defines a value
    std::fs::write(dir.join("c.sema"), r#"(define base-val 10)"#).unwrap();

    // b.sema loads c.sema and defines something on top
    std::fs::write(
        dir.join("b.sema"),
        r#"(load "c.sema") (define doubled (* base-val 2))"#,
    )
    .unwrap();

    // a.sema loads b.sema and prints result
    std::fs::write(dir.join("a.sema"), r#"(load "b.sema") (println doubled)"#).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("a.sema").to_str().unwrap(),
            "-o",
            dir.join("app").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Remove all source files
    std::fs::remove_file(dir.join("a.sema")).unwrap();
    std::fs::remove_file(dir.join("b.sema")).unwrap();
    std::fs::remove_file(dir.join("c.sema")).unwrap();

    let run = std::process::Command::new(dir.join("app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "20");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_sema_build_with_subdirectory_loads() {
    let dir = build_test_dir("subdir-loads");
    std::fs::create_dir_all(dir.join("lib")).unwrap();

    // lib/helpers.sema
    std::fs::write(
        dir.join("lib/helpers.sema"),
        r#"(define (add a b) (+ a b))"#,
    )
    .unwrap();

    // main.sema loads from subdirectory
    std::fs::write(
        dir.join("main.sema"),
        r#"(load "lib/helpers.sema") (println (add 3 4))"#,
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("main.sema").to_str().unwrap(),
            "-o",
            dir.join("app").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(dir.join("lib")).unwrap();
    std::fs::remove_file(dir.join("main.sema")).unwrap();

    let run = std::process::Command::new(dir.join("app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "7");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_sema_build_mixed_load_and_import() {
    let dir = build_test_dir("mixed");
    std::fs::create_dir_all(dir.join("lib")).unwrap();

    // Module for import
    std::fs::write(
        dir.join("lib/math.sema"),
        r#"(module math (export square) (define (square x) (* x x)))"#,
    )
    .unwrap();

    // Helper for load
    std::fs::write(
        dir.join("util.sema"),
        r#"(define (fmt-result n) (format "result: ~a" n))"#,
    )
    .unwrap();

    // Main uses both
    std::fs::write(
        dir.join("main.sema"),
        r#"(load "util.sema") (import "lib/math.sema") (println (fmt-result (square 5)))"#,
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "build",
            dir.join("main.sema").to_str().unwrap(),
            "-o",
            dir.join("app").to_str().unwrap(),
        ])
        .output()
        .expect("failed to run sema build");

    assert!(
        output.status.success(),
        "sema build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(dir.join("lib")).unwrap();
    std::fs::remove_file(dir.join("util.sema")).unwrap();
    std::fs::remove_file(dir.join("main.sema")).unwrap();

    let run = std::process::Command::new(dir.join("app"))
        .output()
        .expect("failed to run bundled executable");

    assert!(
        run.status.success(),
        "bundled executable failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "result: 25");

    let _ = std::fs::remove_dir_all(&dir);
}

// All package import tests share a single SEMA_HOME env var and must run
// sequentially in a single thread to avoid races with other tests.
#[test]
fn test_package_imports() {
    std::thread::spawn(|| {
        let tmp = std::env::temp_dir().join(format!("sema-pkg-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);

        // Set up all fake packages in one SEMA_HOME
        let mylib = tmp.join("packages/github.com/test/mylib");
        std::fs::create_dir_all(&mylib).unwrap();
        std::fs::write(
            mylib.join("package.sema"),
            "(module mylib (export greet) (define (greet name) (string-append \"hello \" name)))",
        )
        .unwrap();

        let mathlib = tmp.join("packages/github.com/test/mathlib");
        std::fs::create_dir_all(&mathlib).unwrap();
        std::fs::write(
            mathlib.join("package.sema"),
            "(module mathlib (export square cube) (define (square x) (* x x)) (define (cube x) (* x x x)) (define (internal) 999))",
        )
        .unwrap();

        let custom = tmp.join("packages/github.com/test/custom");
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("sema.toml"), "entrypoint = \"lib.sema\"\n").unwrap();
        std::fs::write(
            custom.join("lib.sema"),
            "(module custom (export answer) (define (answer) 42))",
        )
        .unwrap();

        let cached = tmp.join("packages/github.com/test/cached");
        std::fs::create_dir_all(&cached).unwrap();
        std::fs::write(
            cached.join("package.sema"),
            "(module cached (export val) (define val 99))",
        )
        .unwrap();

        std::env::set_var("SEMA_HOME", &tmp);

        // --- Basic package import ---
        {
            let interp = Interpreter::new();
            let result = interp
                .eval_str(r#"(begin (import "github.com/test/mylib") (greet "world"))"#)
                .unwrap();
            assert_eq!(result, Value::string("hello world"), "basic package import");
        }

        // --- Selective import ---
        {
            let interp = Interpreter::new();
            let result = interp
                .eval_str(r#"(begin (import "github.com/test/mathlib" square) (square 5))"#)
                .unwrap();
            assert_eq!(result, Value::int(25), "selective package import");
        }

        // --- Custom entrypoint via sema.toml ---
        {
            let interp = Interpreter::new();
            let result = interp
                .eval_str(r#"(begin (import "github.com/test/custom") (answer))"#)
                .unwrap();
            assert_eq!(result, Value::int(42), "custom entrypoint import");
        }

        // --- Package not found error ---
        {
            let interp = Interpreter::new();
            let err = interp
                .eval_str(r#"(import "github.com/nonexistent/pkg")"#)
                .unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("package not found"), "not found error: got {msg}");
        }

        // --- Path traversal rejected ---
        {
            let interp = Interpreter::new();
            let err = interp
                .eval_str(r#"(import "github.com/../../etc/passwd")"#)
                .unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("path traversal"), "traversal rejected: got {msg}");
        }

        // --- Module caching (import twice) ---
        {
            let interp = Interpreter::new();
            let result = interp
                .eval_str(r#"(begin
                    (import "github.com/test/cached")
                    (import "github.com/test/cached")
                    val)"#)
                .unwrap();
            assert_eq!(result, Value::int(99), "cached import");
        }

        // Cleanup
        std::env::remove_var("SEMA_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    })
    .join()
    .unwrap();
}

// ── sema completions (shell completion generation) ────────────────

#[test]
fn test_completions_all_shells_generate_without_panic() {
    // Regression guard: a hidden `__complete-doc-symbols` clap subcommand once
    // made clap_complete's bash generator panic. Every shell must generate a
    // non-empty script and exit 0.
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = sema_cmd()
            .args(["completions", shell])
            .output()
            .expect("failed to run sema completions");
        assert!(
            output.status.success(),
            "sema completions {shell} did not exit 0; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.stdout.is_empty(),
            "sema completions {shell} produced no script"
        );
    }
}

#[test]
fn test_completions_bash_zsh_fish_wire_dynamic_doc_hook() {
    // The interactive shells must include the dynamic doc-symbol hook that calls
    // back into `sema __complete-doc-symbols`; bash must also define the wrapper.
    for shell in ["bash", "zsh", "fish"] {
        let output = sema_cmd()
            .args(["completions", shell])
            .output()
            .expect("failed to run sema completions");
        let script = String::from_utf8_lossy(&output.stdout);
        assert!(
            script.contains("__complete-doc-symbols"),
            "{shell} completion missing dynamic doc-symbol hook"
        );
    }
    let bash = sema_cmd().args(["completions", "bash"]).output().unwrap();
    let bash = String::from_utf8_lossy(&bash.stdout);
    assert!(
        bash.contains("_sema_doc_complete"),
        "bash completion missing the doc-completion wrapper function"
    );
}

// ── sema eval subcommand ──────────────────────────────────────────

#[test]
fn test_doc_show_builtin() {
    let output = sema_cmd()
        .args(["doc", "string/split"])
        .output()
        .expect("failed to run sema doc");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("string/split"), "stdout: {stdout}");
    assert!(
        stdout.contains("Split a string by a literal delimiter"),
        "stdout: {stdout}"
    );
}

#[test]
fn test_doc_search_finds_builtin() {
    let output = sema_cmd()
        .args(["doc", "search", "split", "a", "string"])
        .output()
        .expect("failed to run sema doc search");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("string/split"), "stdout: {stdout}");
}

#[test]
fn test_doc_apropos_finds_name_matches() {
    let output = sema_cmd()
        .args(["doc", "apropos", "string/spl"])
        .output()
        .expect("failed to run sema doc apropos");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("string/split") || stdout.contains("string-split"),
        "stdout: {stdout}"
    );
}

#[test]
fn test_complete_doc_symbols_filters_prefix() {
    let output = sema_cmd()
        .args(["__complete-doc-symbols", "string/spl"])
        .output()
        .expect("failed to run sema __complete-doc-symbols");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.lines().any(|line| line == "string/split"));
    assert!(!stdout.lines().any(|line| line == "map"));
}

#[test]
fn test_eval_expr_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(+ 1 2)", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON output");
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "3");
    assert_eq!(json["stdout"], "");
    assert_eq!(json["stderr"], "");
    assert!(json["elapsedMs"].as_u64().is_some());
}

#[test]
fn test_eval_stdin_json() {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["eval", "--stdin", "--json", "--no-llm"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sema eval");
    child.stdin.take().unwrap().write_all(b"(* 6 7)").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "42");
}

#[test]
fn test_eval_error_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(/ 1 0)", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    // --json mode exits 0 even on eval errors
    assert!(output.status.success(), "expected exit 0 for --json error");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert!(!json["error"]["message"].as_str().unwrap().is_empty());
}

#[test]
fn test_eval_expr_no_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(+ 10 20)", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().contains("30"), "expected 30, got: {stdout}");
}

#[test]
fn test_eval_nil_result_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(define x 42)", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert!(
        json["value"].is_null(),
        "define returns nil, value should be null"
    );
}

#[test]
fn test_eval_stdin_multi_form() {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["eval", "--stdin", "--json", "--no-llm"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn sema eval");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"(define pi 3.14)\n(define (area r) (* pi r r))\n(area 10)")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "314.0");
}

#[test]
fn test_eval_no_input_error() {
    let output = sema_cmd()
        .args(["eval", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    assert!(
        !output.status.success(),
        "should fail without --stdin or --expr"
    );
}

#[test]
fn test_eval_sandbox_blocks_shell() {
    let output = sema_cmd()
        .args([
            "eval",
            "--expr",
            "(shell \"echo hi\")",
            "--json",
            "--no-llm",
            "--sandbox",
            "strict",
        ])
        .output()
        .expect("failed to run sema eval");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(
        msg.to_lowercase().contains("sandbox")
            || msg.to_lowercase().contains("denied")
            || msg.to_lowercase().contains("not permitted"),
        "expected sandbox error, got: {msg}"
    );
}

#[test]
fn test_eval_parse_error_json() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(+ 1", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    // Parse error in --json mode should still exit 0
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    assert!(!json["error"]["message"].as_str().unwrap().is_empty());
}

#[test]
fn test_eval_stdout_captured_in_json() {
    let output = sema_cmd()
        .args([
            "eval",
            "--expr",
            "(println \"hello world\") (+ 1 2)",
            "--json",
            "--no-llm",
        ])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("stdout must be valid JSON even when user code prints");
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "3");
    assert_eq!(json["stdout"], "hello world\n");
    assert_eq!(json["stderr"], "");
}

#[test]
fn test_eval_stderr_captured_in_json() {
    let output = sema_cmd()
        .args([
            "eval",
            "--expr",
            "(print-error \"oops\") 42",
            "--json",
            "--no-llm",
        ])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "42");
    assert_eq!(json["stderr"], "oops");
}

#[test]
fn test_eval_error_has_line_and_col() {
    let output = sema_cmd()
        .args(["eval", "--expr", "(+ 1", "--json", "--no-llm"])
        .output()
        .expect("failed to run sema eval");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    // Parse errors should include line info
    assert!(
        json["error"]["line"].as_u64().is_some(),
        "expected line in error: {json}"
    );
}

#[test]
fn test_eval_virtual_path_does_not_crash() {
    let output = sema_cmd()
        .args([
            "eval",
            "--expr",
            "(+ 1 2)",
            "--json",
            "--no-llm",
            "--path",
            "/nonexistent/untitled.sema",
        ])
        .output()
        .expect("failed to run sema eval");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["value"], "3");
}

// ── Stream integration tests ─────────────────────────────────────

#[test]
fn test_stream_file_roundtrip() {
    let dir = std::env::temp_dir().join("sema-stream-roundtrip");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("roundtrip.txt");
    let path_str = path.display();

    // Write via stream
    eval(&format!(
        r#"(let ((s (stream/open-output "{path_str}")))
             (stream/write-string s "hello streams")
             (stream/close s))"#
    ));

    // Read back via stream
    assert_eq!(
        eval(&format!(
            r#"(let ((s (stream/open-input "{path_str}")))
                 (let ((data (stream/read-all s)))
                   (stream/close s)
                   (utf8->string data)))"#
        )),
        Value::string("hello streams")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_stream_file_read_line() {
    let dir = std::env::temp_dir().join("sema-stream-readline");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("lines.txt");
    std::fs::write(&path, "line1\nline2\nline3\n").unwrap();
    let path_str = path.display();

    assert_eq!(
        eval(&format!(
            r#"(let ((s (stream/open-input "{path_str}")))
                 (let ((a (stream/read-line s))
                       (b (stream/read-line s))
                       (c (stream/read-line s)))
                   (stream/close s)
                   (list a b c)))"#
        )),
        Value::list(vec![
            Value::string("line1"),
            Value::string("line2"),
            Value::string("line3"),
        ])
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_stream_file_read_line_crlf() {
    let dir = std::env::temp_dir().join("sema-stream-crlf");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("crlf.txt");
    std::fs::write(&path, "a\r\nb\r\n").unwrap();
    let path_str = path.display();

    assert_eq!(
        eval(&format!(
            r#"(let ((s (stream/open-input "{path_str}")))
                 (let ((a (stream/read-line s)))
                   (stream/close s)
                   a))"#
        )),
        Value::string("a")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_stream_copy() {
    let dir = std::env::temp_dir().join("sema-stream-copy");
    let _ = std::fs::create_dir_all(&dir);
    let src = dir.join("copy-src.txt");
    let dst = dir.join("copy-dst.txt");
    std::fs::write(&src, "copy me").unwrap();
    let src_str = src.display();
    let dst_str = dst.display();

    eval(&format!(
        r#"(let ((in (stream/open-input "{src_str}"))
                 (out (stream/open-output "{dst_str}")))
             (stream/copy in out)
             (stream/close in)
             (stream/close out))"#
    ));

    assert_eq!(std::fs::read_to_string(&dst).unwrap(), "copy me");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_stream_stdio_are_streams() {
    assert_eq!(eval("(stream? *stdout*)"), Value::bool(true));
    assert_eq!(eval("(stream? *stderr*)"), Value::bool(true));
    assert_eq!(eval("(stream? *stdin*)"), Value::bool(true));
    assert_eq!(eval("(stream/type *stdout*)"), Value::string("stdout"));
    assert_eq!(eval("(stream/type *stdin*)"), Value::string("stdin"));
    assert_eq!(eval("(stream/writable? *stdout*)"), Value::bool(true));
    assert_eq!(eval("(stream/readable? *stdout*)"), Value::bool(false));
    assert_eq!(eval("(stream/readable? *stdin*)"), Value::bool(true));
}

#[test]
fn test_stream_read_closed_file() {
    let dir = std::env::temp_dir().join("sema-stream-closed");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("closed.txt");
    std::fs::write(&path, "data").unwrap();
    let path_str = path.display();

    let interp = Interpreter::new();
    let result = interp.eval_str(&format!(
        r#"(let ((s (stream/open-input "{path_str}")))
             (stream/close s)
             (stream/read s 1))"#
    ));
    assert!(result.is_err(), "reading closed stream should error");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_stream_write_string_to_file() {
    let dir = std::env::temp_dir().join("sema-stream-writestr");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("write-string.txt");
    let path_str = path.display();

    eval(&format!(
        r#"(let ((s (stream/open-output "{path_str}")))
             (stream/write-string s "hello ")
             (stream/write-string s "world")
             (stream/close s))"#
    ));

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_with_stream_file() {
    let dir = std::env::temp_dir().join("sema-stream-withmacro");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("with.txt");
    std::fs::write(&path, "via macro").unwrap();
    let path_str = path.display();

    assert_eq!(
        eval(&format!(
            r#"(with-stream (s (stream/open-input "{path_str}"))
                 (utf8->string (stream/read-all s)))"#
        )),
        Value::string("via macro")
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_with_stream_error_cleanup() {
    // with-stream should close the stream even when the body throws
    let dir = std::env::temp_dir().join("sema-stream-errcleanup");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("err.txt");
    std::fs::write(&path, "data").unwrap();
    let path_str = path.display();

    let interp = Interpreter::new();
    // The body throws, but with-stream should still close the stream
    let result = interp.eval_str(&format!(
        r#"(try
             (with-stream (s (stream/open-input "{path_str}"))
               (throw "oops"))
             (catch e "caught"))"#
    ));
    assert_eq!(result.unwrap(), Value::string("caught"));

    let _ = std::fs::remove_dir_all(&dir);
}

// --- SQLite tests ---

#[test]
fn test_db_open_memory_and_close() {
    let interp = Interpreter::new();
    let handle = interp.eval_str(r#"(db/open-memory "test-mem")"#).unwrap();
    assert_eq!(handle, Value::string("test-mem"));
    interp.eval_str(r#"(db/close "test-mem")"#).unwrap();
}

#[test]
fn test_db_exec_and_query() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "eq")"#).unwrap();
    interp
        .eval_str(r#"(db/exec "eq" "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")"#)
        .unwrap();
    let affected = interp
        .eval_str(r#"(db/exec "eq" "INSERT INTO users (name, age) VALUES (?, ?)" "Alice" 30)"#)
        .unwrap();
    assert_eq!(affected, Value::int(1));
    interp
        .eval_str(r#"(db/exec "eq" "INSERT INTO users (name, age) VALUES (?, ?)" "Bob" 25)"#)
        .unwrap();
    let rows = interp
        .eval_str(r#"(db/query "eq" "SELECT name, age FROM users ORDER BY name")"#)
        .unwrap();
    let list = rows.as_list().unwrap();
    assert_eq!(list.len(), 2);
    let alice = list[0].as_map_ref().unwrap();
    assert_eq!(
        alice.get(&Value::keyword("name")).unwrap(),
        &Value::string("Alice")
    );
    assert_eq!(alice.get(&Value::keyword("age")).unwrap(), &Value::int(30));
    let bob = list[1].as_map_ref().unwrap();
    assert_eq!(
        bob.get(&Value::keyword("name")).unwrap(),
        &Value::string("Bob")
    );
    interp.eval_str(r#"(db/close "eq")"#).unwrap();
}

#[test]
fn test_db_query_with_params() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "qp")"#).unwrap();
    interp
        .eval_str(r#"(db/exec "qp" "CREATE TABLE items (id INTEGER PRIMARY KEY, value REAL)")"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "qp" "INSERT INTO items (value) VALUES (?)" 3.14)"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "qp" "INSERT INTO items (value) VALUES (?)" 2.72)"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "qp" "INSERT INTO items (value) VALUES (?)" 1.0)"#)
        .unwrap();
    let rows = interp
        .eval_str(r#"(db/query "qp" "SELECT value FROM items WHERE value > ?" 2.0)"#)
        .unwrap();
    let list = rows.as_list().unwrap();
    assert_eq!(list.len(), 2);
    interp.eval_str(r#"(db/close "qp")"#).unwrap();
}

#[test]
fn test_db_query_one() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "qo")"#).unwrap();
    interp
        .eval_str(r#"(db/exec "qo" "CREATE TABLE kv (key TEXT PRIMARY KEY, val TEXT)")"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "qo" "INSERT INTO kv VALUES (?, ?)" "name" "Alice")"#)
        .unwrap();
    let result = interp
        .eval_str(r#"(db/query-one "qo" "SELECT val FROM kv WHERE key = ?" "name")"#)
        .unwrap();
    let row = result.as_map_ref().unwrap();
    assert_eq!(
        row.get(&Value::keyword("val")).unwrap(),
        &Value::string("Alice")
    );
    let missing = interp
        .eval_str(r#"(db/query-one "qo" "SELECT val FROM kv WHERE key = ?" "nope")"#)
        .unwrap();
    assert!(missing.is_nil());
    interp.eval_str(r#"(db/close "qo")"#).unwrap();
}

#[test]
fn test_db_last_insert_id() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "lid")"#).unwrap();
    interp
        .eval_str(r#"(db/exec "lid" "CREATE TABLE t (id INTEGER PRIMARY KEY)")"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "lid" "INSERT INTO t DEFAULT VALUES")"#)
        .unwrap();
    let id = interp.eval_str(r#"(db/last-insert-id "lid")"#).unwrap();
    assert_eq!(id, Value::int(1));
    interp
        .eval_str(r#"(db/exec "lid" "INSERT INTO t DEFAULT VALUES")"#)
        .unwrap();
    let id2 = interp.eval_str(r#"(db/last-insert-id "lid")"#).unwrap();
    assert_eq!(id2, Value::int(2));
    interp.eval_str(r#"(db/close "lid")"#).unwrap();
}

#[test]
fn test_db_tables() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "tbl")"#).unwrap();
    interp
        .eval_str(r#"(db/exec "tbl" "CREATE TABLE alpha (id INTEGER)")"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "tbl" "CREATE TABLE beta (id INTEGER)")"#)
        .unwrap();
    let tables = interp.eval_str(r#"(db/tables "tbl")"#).unwrap();
    let list = tables.as_list().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0], Value::string("alpha"));
    assert_eq!(list[1], Value::string("beta"));
    interp.eval_str(r#"(db/close "tbl")"#).unwrap();
}

#[test]
fn test_db_exec_batch() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "batch")"#).unwrap();
    interp
        .eval_str(r#"(db/exec-batch "batch" "CREATE TABLE a (id INTEGER); CREATE TABLE b (id INTEGER);")"#)
        .unwrap();
    let tables = interp.eval_str(r#"(db/tables "batch")"#).unwrap();
    let list = tables.as_list().unwrap();
    assert_eq!(list.len(), 2);
    interp.eval_str(r#"(db/close "batch")"#).unwrap();
}

#[test]
fn test_db_null_values() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "nv")"#).unwrap();
    interp
        .eval_str(r#"(db/exec "nv" "CREATE TABLE t (a TEXT, b INTEGER)")"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "nv" "INSERT INTO t VALUES (?, ?)" nil 42)"#)
        .unwrap();
    let row = interp
        .eval_str(r#"(db/query-one "nv" "SELECT a, b FROM t")"#)
        .unwrap();
    let map = row.as_map_ref().unwrap();
    assert!(map.get(&Value::keyword("a")).unwrap().is_nil());
    assert_eq!(map.get(&Value::keyword("b")).unwrap(), &Value::int(42));
    interp.eval_str(r#"(db/close "nv")"#).unwrap();
}

#[test]
fn test_db_open_file() {
    let tmp = std::env::temp_dir().join("sema-db-test-file.db");
    let path = tmp.to_str().unwrap();
    let _ = std::fs::remove_file(&tmp);
    let interp = Interpreter::new();
    interp.eval_str(&format!(r#"(db/open "{path}")"#)).unwrap();
    interp
        .eval_str(&format!(r#"(db/exec "{path}" "CREATE TABLE t (v TEXT)")"#))
        .unwrap();
    interp
        .eval_str(&format!(
            r#"(db/exec "{path}" "INSERT INTO t VALUES (?)" "hello")"#
        ))
        .unwrap();
    interp.eval_str(&format!(r#"(db/close "{path}")"#)).unwrap();
    // Reopen and verify persistence
    interp.eval_str(&format!(r#"(db/open "{path}")"#)).unwrap();
    let result = interp
        .eval_str(&format!(r#"(db/query-one "{path}" "SELECT v FROM t")"#))
        .unwrap();
    let map = result.as_map_ref().unwrap();
    assert_eq!(
        map.get(&Value::keyword("v")).unwrap(),
        &Value::string("hello")
    );
    interp.eval_str(&format!(r#"(db/close "{path}")"#)).unwrap();
    let _ = std::fs::remove_file(&tmp);
}

#[test]
fn test_db_error_no_open() {
    let interp = Interpreter::new();
    let result = interp.eval_str(r#"(db/query "nonexistent" "SELECT 1")"#);
    assert!(result.is_err());
}

#[test]
fn test_db_error_bad_sql() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "bad")"#).unwrap();
    let result = interp.eval_str(r#"(db/exec "bad" "NOT VALID SQL")"#);
    assert!(result.is_err());
    interp.eval_str(r#"(db/close "bad")"#).unwrap();
}

#[test]
fn test_db_foreign_keys() {
    let interp = Interpreter::new();
    interp.eval_str(r#"(db/open-memory "fk")"#).unwrap();
    interp
        .eval_str(r#"(db/exec "fk" "CREATE TABLE parent (id INTEGER PRIMARY KEY)")"#)
        .unwrap();
    interp
        .eval_str(r#"(db/exec "fk" "CREATE TABLE child (id INTEGER, pid INTEGER REFERENCES parent(id))")"#)
        .unwrap();
    // Inserting a child with non-existent parent should fail because foreign_keys=ON
    let result = interp.eval_str(r#"(db/exec "fk" "INSERT INTO child VALUES (1, 999)")"#);
    assert!(result.is_err());
    interp.eval_str(r#"(db/close "fk")"#).unwrap();
}

// === Sandbox: http/file gated under fs-read (C5) ===

#[test]
fn test_sandbox_http_file_denied_under_fs_read() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(http/file "/tmp/anything")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

// === Sandbox: db/exec and friends gated (C6) ===

#[test]
fn test_sandbox_db_exec_denied_under_fs_write() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_WRITE);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(db/exec "nonexistent" "CREATE TABLE t (a INTEGER)")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());

    let result = interp.eval_str(r#"(db/exec-batch "nonexistent" "SELECT 1")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

#[test]
fn test_sandbox_db_query_denied_under_fs_read() {
    let sandbox = sema_core::Sandbox::deny(sema_core::Caps::FS_READ);
    let interp = Interpreter::new_with_sandbox(&sandbox);
    let result = interp.eval_str(r#"(db/query "nonexistent" "SELECT 1")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());

    let result = interp.eval_str(r#"(db/query-one "nonexistent" "SELECT 1")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());

    let result = interp.eval_str(r#"(db/tables "nonexistent")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());

    let result = interp.eval_str(r#"(db/last-insert-id "nonexistent")"#);
    assert!(result.is_err());
    assert_permission_denied(&result.unwrap_err());
}

// ── Regression: top-level (async ...) drains scheduler at exit (bug C2) ───────
//
// A top-level `(async ...)` form spawns a task whose side effects would
// silently vanish on exit unless the scheduler is drained. The CLI now
// invokes the scheduler after a successful top-level eval to flush any
// pending work.
#[test]
fn test_cli_top_level_async_drains_scheduler() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args([
            "--no-llm",
            "-e",
            r#"(begin (async (println "side effect!")) :end)"#,
        ])
        .output()
        .expect("failed to run sema");

    assert!(
        output.status.success(),
        "sema -e exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("side effect!"),
        "expected top-level async side effect to run, got stdout: {stdout}"
    );
    assert!(
        stdout.contains(":end"),
        "expected final expression value, got stdout: {stdout}"
    );
}

// ── io/read-line and io/read-stdin: subprocess + piped stdin ──────────────
//
// These tests pin down the EOF-on-stdin contract:
//   * (io/read-line) returns the next line without trailing newline, or
//     nil on EOF.
//   * (io/eof?) flips to #t after read-line / read-stdin observed EOF.
//   * (io/read-stdin) consumes all remaining stdin into a string.

fn run_sema_with_stdin(program: &str, stdin_input: &[u8]) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-e", program])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sema");
    if !stdin_input.is_empty() {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stdin_input)
            .expect("failed to write stdin");
    }
    // Drop stdin to signal EOF.
    drop(child.stdin.take());
    child.wait_with_output().expect("failed to wait for sema")
}

#[test]
fn test_io_read_line_returns_string_without_newline() {
    let output = run_sema_with_stdin(r#"(println (io/read-line))"#, b"hello world\n");
    assert!(
        output.status.success(),
        "sema exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // println adds its own newline; read-line should have stripped the input \n.
    assert_eq!(stdout.trim_end_matches('\n'), "hello world");
}

#[test]
fn test_io_read_line_eof_returns_nil_and_sets_eof_flag() {
    // Pipe EOF immediately: (io/read-line) should be nil; (io/eof?) should be #t.
    let output = run_sema_with_stdin(
        r#"(let ((v (io/read-line))) (println (if (nil? v) "nil-ok" "BAD")) (println (io/eof?)))"#,
        b"",
    );
    assert!(
        output.status.success(),
        "sema exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("nil-ok"),
        "expected nil-ok marker, got: {stdout}"
    );
    assert!(
        stdout.contains("#t"),
        "expected (io/eof?) to be #t after EOF, got: {stdout}"
    );
}

#[test]
fn test_io_read_line_empty_line_then_eof() {
    // First read returns "" (empty line), second read returns nil.
    let output = run_sema_with_stdin(
        r#"
        (let ((first (io/read-line))
              (second (io/read-line)))
          (println (if (and (string? first) (= (string-length first) 0)) "empty-ok" "BAD-FIRST"))
          (println (if (nil? second) "nil-ok" "BAD-SECOND")))
        "#,
        b"\n",
    );
    assert!(
        output.status.success(),
        "sema exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("empty-ok"), "got: {stdout}");
    assert!(stdout.contains("nil-ok"), "got: {stdout}");
}

#[test]
fn test_io_read_line_alias_legacy_name() {
    // `read-line` is the canonical name with `io/read-line` as an alias.
    // Pin that the alias resolves to the same function.
    let output = run_sema_with_stdin(r#"(println (read-line))"#, b"alias-works\n");
    assert!(
        output.status.success(),
        "sema exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim_end_matches('\n'), "alias-works");
}

#[test]
fn test_io_read_stdin_returns_full_input() {
    let payload = "line1\nline2\nline3\n";
    let output = run_sema_with_stdin(r#"(display (io/read-stdin))"#, payload.as_bytes());
    assert!(
        output.status.success(),
        "sema exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, payload);
}

#[test]
fn test_io_read_stdin_empty_returns_empty_string_and_sets_eof() {
    let output = run_sema_with_stdin(
        r#"(let ((s (io/read-stdin)))
             (println (if (and (string? s) (= (string-length s) 0)) "empty-ok" "BAD"))
             (println (io/eof?)))"#,
        b"",
    );
    assert!(
        output.status.success(),
        "sema exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("empty-ok"), "got: {stdout}");
    assert!(
        stdout.contains("#t"),
        "expected (io/eof?) to be #t after read-stdin on empty input, got: {stdout}"
    );
}

// ===========================================================================
// Wave 6 polish: CLI / REPL ergonomics
// ===========================================================================

/// Spawn the REPL with a piped stdin payload and the `-q` (quiet) flag.
fn run_repl_with_input(input: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", "-q"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn sema");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("failed to write stdin");
    drop(child.stdin.take());
    child.wait_with_output().expect("failed to wait for sema")
}

#[test]
fn test_t3_notebook_run_prints_stdout() {
    // Create a tiny notebook that prints to stdout via println.
    let dir = std::env::temp_dir().join(format!("sema-t3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let nb = dir.join("nb.sema-nb");
    let body = r#"{
      "version": 1,
      "metadata": {"title": "t3", "created": "2026-05-04T00:00:00Z", "modified": "2026-05-04T00:00:00Z", "sema_version": "1.14.3"},
      "cells": [
        {"id": "c1", "type": "code", "source": "(println \"hello-from-notebook\")"}
      ]
    }"#;
    std::fs::write(&nb, body).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["notebook", "run", nb.to_str().unwrap()])
        .output()
        .expect("failed to spawn sema notebook run");
    assert!(
        output.status.success(),
        "exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("(stdout)") && stdout.contains("hello-from-notebook"),
        "expected captured stdout to appear in `notebook run` output, got: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_u1_file_not_found_wording() {
    // Compile / run / disasm / fmt / ast: each should produce a consistent
    // `file not found:` message rather than a raw OS error.
    let missing = temp_path("this-file-definitely-does-not-exist-sema-u1.sema");
    let missing = missing.as_str();

    for subcmd in &["compile", "fmt", "disasm"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
            .args([subcmd, missing])
            .output()
            .expect("failed to spawn sema");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("file not found"),
            "{subcmd}: expected consistent 'file not found' wording, got: {stderr}"
        );
    }

    // `run` (default file mode) is also covered.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["--no-llm", missing])
        .output()
        .expect("failed to spawn sema");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("file not found"),
        "default-run: expected 'file not found' wording, got: {stderr}"
    );
}

#[test]
fn test_u2_notebook_without_subcommand_shows_error_and_help() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .arg("notebook")
        .output()
        .expect("failed to spawn sema");
    assert!(
        !output.status.success(),
        "expected non-zero exit when invoking `sema notebook` with no subcommand"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("error")
            || stderr.contains("Usage")
            || stderr.contains("usage"),
        "expected clap to emit an error/usage message, got: {stderr}"
    );
}

#[test]
fn test_u3_build_preflight_permission_denied() {
    // Use a path that almost certainly cannot be written from a normal user
    // process: /sema-u3-output. On a sandboxed test runner /no-such-parent
    // returning "output directory does not exist" is equivalent — either
    // message should be emitted before any [1/5] step.
    let dir = std::env::temp_dir().join(format!("sema-u3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("hello.sema");
    std::fs::write(&src, r#"(println "x")"#).unwrap();

    let unwritable_out = "/no/such/parent/dir/u3-out";

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sema"))
        .args(["build", src.to_str().unwrap(), "-o", unwritable_out])
        .output()
        .expect("failed to spawn sema build");
    assert!(
        !output.status.success(),
        "expected non-zero exit when output dir is unwritable"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("output directory does not exist") || stderr.contains("permission denied"),
        "expected pre-flight write error, got: {stderr}"
    );
    // Pre-flight must happen before any compile step, so we shouldn't see "[5/5]".
    assert!(
        !stderr.contains("[5/5]"),
        "expected pre-flight to fire before step 5, got: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_u4_repl_eof_unterminated() {
    // Pipe an unterminated form then EOF.  REPL must complain and exit
    // non-zero rather than silently dropping the input.
    let output = run_repl_with_input("(+ 1\n");
    assert!(
        !output.status.success(),
        "expected non-zero exit on unterminated EOF"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unterminated input at EOF"),
        "expected unterminated-input error, got: {stderr}"
    );
}

#[test]
fn test_u5_repl_define_feedback() {
    // Top-level `(define x 1)` evaluates to nil; the REPL should still print
    // `; defined x` so the user knows something happened.
    let output = run_repl_with_input("(define x 41)\n,quit\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("defined x"),
        "expected '; defined x' feedback, got stdout: {stdout}"
    );
}

#[test]
fn test_u7_repl_env_hides_prelude() {
    // Define a single user binding and confirm `,env` shows it but not the
    // hundreds of prelude / history entries.
    let output = run_repl_with_input("(define my-thing 7)\n,env\n,quit\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("my-thing"),
        "expected user binding to appear in ,env, got: {stdout}"
    );
    // History slots (*1, *2, *3, *e) must be hidden.
    assert!(
        !stdout.contains("*1 ="),
        "expected ,env to hide history slot *1, got: {stdout}"
    );
}

#[test]
fn test_u8_repl_bare_quit_exits() {
    // `quit` (no comma) should exit the REPL the same way `,quit` does.
    let output = run_repl_with_input("quit\n");
    assert!(
        output.status.success(),
        "bare `quit` should exit cleanly, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Same for `exit` and `:q`.
    let output = run_repl_with_input("exit\n");
    assert!(output.status.success(), "bare `exit` should exit cleanly");
    let output = run_repl_with_input(":q\n");
    assert!(output.status.success(), ":q should exit cleanly");
}
