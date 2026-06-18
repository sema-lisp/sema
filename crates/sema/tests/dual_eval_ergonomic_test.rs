mod common;

use sema_core::Value;

// ============================================================
// F-strings — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    fstring_basic: r#"(let ((name "world")) f"hello ${name}")"# => Value::string("hello world"),
    fstring_multiple: r#"(let ((a "foo") (b "bar")) f"${a} and ${b}")"# => Value::string("foo and bar"),
    fstring_expression: r#"f"result: ${(+ 1 2)}""# => Value::string("result: 3"),
    fstring_nested_call: r#"(let ((x 42)) f"the answer is ${x}")"# => Value::string("the answer is 42"),
    fstring_no_interpolation: r#"f"just a string""# => Value::string("just a string"),
    fstring_keyword_access: r#"(let ((m {:name "Ada"})) f"name: ${(:name m)}")"# => Value::string("name: Ada"),
    fstring_escaped_dollar: r#"f"costs \$5""# => Value::string("costs $5"),
    fstring_dollar_without_brace: r#"f"costs $5""# => Value::string("costs $5"),
}

// ============================================================
// Short lambdas — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    short_lambda_basic: "(map #(+ % 1) '(1 2 3))" => Value::list(vec![Value::int(2), Value::int(3), Value::int(4)]),
    short_lambda_square: "(map #(* % %) '(1 2 3 4))" => Value::list(vec![Value::int(1), Value::int(4), Value::int(9), Value::int(16)]),
    short_lambda_filter: "(filter #(> % 3) '(1 2 3 4 5))" => Value::list(vec![Value::int(4), Value::int(5)]),
    short_lambda_two_args: "(#(+ %1 %2) 3 4)" => Value::int(7),
    short_lambda_no_args: "(#(+ 1 2))" => Value::int(3),
    short_lambda_nested_call: r#"(map #(string-length %) '("hi" "hello" "hey"))"# => Value::list(vec![Value::int(2), Value::int(5), Value::int(3)]),
}

// ============================================================
// Threading macros — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    thread_first: "(-> 5 (+ 3) (* 2))" => Value::int(16),
    thread_first_bare_fn: r#"(-> "hello" string-length)"# => Value::int(5),
    thread_last: "(->> (range 1 6) (filter odd?))" => Value::list(vec![Value::int(1), Value::int(3), Value::int(5)]),
    thread_last_pipeline: "(->> (range 1 6) (map (fn (x) (* x x))) (foldl + 0))" => Value::int(55),
    thread_as: "(as-> 5 x (+ x 3) (* x x) (- x 1))" => Value::int(63),
    some_thread_non_nil: r#"(some-> {:a {:b 42}} (get :a) (get :b))"# => Value::int(42),
    some_thread_nil: r#"(some-> {:a nil} (get :a) (get :b))"# => Value::nil(),
}

// ============================================================
// when-let / if-let — dual eval (tree-walker + VM)
// ============================================================

dual_eval_tests! {
    when_let_truthy: "(when-let (x 42) (+ x 1))" => Value::int(43),
    when_let_nil: "(when-let (x nil) (+ x 1))" => Value::nil(),
    if_let_truthy: "(if-let (x 42) (+ x 1) 0)" => Value::int(43),
    if_let_nil: "(if-let (x nil) (+ x 1) 0)" => Value::int(0),
}
