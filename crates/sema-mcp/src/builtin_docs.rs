use std::collections::HashMap;

/// Build the builtin documentation map served by the `docs` MCP tool.
///
/// Stdlib docs come from the canonical `sema-docs` index — the same committed
/// JSON source the LSP and REPL use (`sema_docs::builtin_index()`) — rather than
/// re-parsing the website markdown via `include_str!`. That keeps this crate
/// self-contained (publishable to crates.io; the old `../../../website/...`
/// paths escaped the crate and broke `cargo publish`) and avoids drift between
/// the MCP docs and the rest of the toolchain.
pub fn build_builtin_docs() -> HashMap<String, String> {
    let mut docs = HashMap::new();

    // Stdlib + special-form entries from the canonical sema-docs index.
    // Use the full markdown body (what LSP hover renders); fall back to summary.
    for e in &sema_docs::builtin_index().entries {
        let text = if e.body.trim().is_empty() {
            e.summary.clone()
        } else {
            e.body.clone()
        };
        if text.trim().is_empty() {
            continue;
        }
        for key in std::iter::once(e.name.clone()).chain(e.aliases.iter().cloned()) {
            docs.entry(key).or_insert_with(|| text.clone());
        }
    }

    // Curated, example-first special-form blurbs. These intentionally override
    // the index entries so the `docs` tool returns the concise form it has always
    // returned for special forms.
    let special_forms: &[(&str, &str)] = &[
        ("define", "Define a variable or function.\n\n```sema\n(define x 42)\n(define (square x) (* x x))\n```"),
        ("defun", "Define a named function.\n\nSyntax: `(defun name (params...) body...)`\n\n```sema\n(defun greet (name) (string-append \"Hello, \" name))\n```"),
        ("defn", "Define a named function (alias for `defun`).\n\nSyntax: `(defn name (params...) body...)`\n\n```sema\n(defn add (a b) (+ a b))\n(defn greet (name) (string-append \"Hello, \" name))\n```"),
        ("defmacro", "Define a macro.\n\n```sema\n(defmacro unless (test body) `(if (not ,test) ,body))\n```"),
        ("lambda", "Create an anonymous function.\n\n```sema\n(lambda (x y) (+ x y))\n```"),
        ("fn", "Alias for `lambda`. Create an anonymous function.\n\n```sema\n(fn (x) (* x x))\n```"),
        ("if", "Conditional expression.\n\n```sema\n(if (> x 0) \"positive\" \"non-positive\")\n```"),
        ("cond", "Multi-branch conditional.\n\n```sema\n(cond\n  ((< x 0) \"negative\")\n  ((= x 0) \"zero\")\n  (else \"positive\"))\n```"),
        ("let", "Bind local variables.\n\n```sema\n(let ((x 1) (y 2)) (+ x y))\n```"),
        ("let*", "Bind local variables sequentially (each can refer to previous).\n\n```sema\n(let* ((x 1) (y (+ x 1))) y)  ; => 2\n```"),
        ("letrec", "Bind local variables with mutual recursion.\n\n```sema\n(letrec ((even? (fn (n) (if (= n 0) #t (odd? (- n 1)))))\n         (odd?  (fn (n) (if (= n 0) #f (even? (- n 1))))))\n  (even? 10))\n```"),
        ("begin", "Sequence expressions, returning the last.\n\n```sema\n(begin (println \"hello\") (+ 1 2))  ; => 3\n```"),
        ("set!", "Mutate a variable binding.\n\n```sema\n(define x 1)\n(set! x 2)\nx  ; => 2\n```"),
        ("quote", "Return the expression unevaluated.\n\n```sema\n(quote (1 2 3))  ; => (1 2 3)\n'(1 2 3)         ; => (1 2 3)\n```"),
        ("quasiquote", "Template with unquote splicing.\n\n```sema\n`(1 ,(+ 1 1) 3)  ; => (1 2 3)\n```"),
        ("and", "Short-circuit logical AND.\n\n```sema\n(and #t #t)   ; => #t\n(and #t #f)   ; => #f\n```"),
        ("or", "Short-circuit logical OR.\n\n```sema\n(or #f #t)   ; => #t\n(or #f #f)   ; => #f\n```"),
        ("when", "Execute body when condition is true.\n\n```sema\n(when (> x 0) (println \"positive\"))\n```"),
        ("unless", "Execute body when condition is false.\n\n```sema\n(unless (> x 0) (println \"non-positive\"))\n```"),
        ("while", "Loop while condition is true.\n\n```sema\n(define i 0)\n(while (< i 5) (set! i (+ i 1)))\n```"),
        ("do", "Iteration construct with step expressions.\n\n```sema\n(do ((i 0 (+ i 1)))\n    ((= i 5) i))\n```"),
        ("match", "Pattern matching.\n\n```sema\n(match x\n  (0 \"zero\")\n  ((? number?) \"number\")\n  (_ \"other\"))\n```"),
        ("case", "Value-based dispatch.\n\n```sema\n(case x\n  ((1) \"one\")\n  ((2 3) \"two or three\")\n  (else \"other\"))\n```"),
        ("try", "Exception handling.\n\n```sema\n(try\n  (/ 1 0)\n  (catch e (println \"Error:\" e)))\n```"),
        ("throw", "Raise an exception.\n\n```sema\n(throw \"something went wrong\")\n```"),
        ("import", "Import a module.\n\n```sema\n(import \"utils.sema\")\n(import \"lib.sema\" (helper-fn other-fn))\n```"),
        ("load", "Load and evaluate a file.\n\n```sema\n(load \"config.sema\")\n```"),
        ("export", "Declare exported symbols from a module.\n\n```sema\n(export my-fn my-var)\n```"),
        ("delay", "Create a lazy promise.\n\n```sema\n(define p (delay (expensive-computation)))\n```"),
        ("force", "Force evaluation of a delayed promise.\n\n```sema\n(force p)  ; evaluates the delayed computation\n```"),
        ("eval", "Evaluate an expression at runtime.\n\n```sema\n(eval '(+ 1 2))  ; => 3\n```"),
        ("defagent", "Define an LLM agent.\n\n```sema\n(defagent my-agent\n  :model \"claude-sonnet\"\n  :system \"You are helpful.\")\n```"),
        ("deftool", "Define a tool for an LLM agent.\n\n```sema\n(deftool get-weather (location)\n  \"Get weather for a location\"\n  (http/get (format \"https://api.weather.com/~a\" location)))\n```"),
        ("prompt", "Send a prompt to an LLM.\n\n```sema\n(prompt \"Explain recursion in one sentence\")\n```"),
        ("for", "Iterate with bindings.\n\n```sema\n(for ((x (range 5)))\n  (println x))\n```"),
        ("for/list", "Collect iteration results into a list.\n\n```sema\n(for/list ((x (range 5)))\n  (* x x))  ; => (0 1 4 9 16)\n```"),
        ("for/map", "Collect iteration results into a map.\n\n```sema\n(for/map ((x '(1 2 3)))\n  (values x (* x x)))  ; => {1 1, 2 4, 3 9}\n```"),
        ("for/filter", "Filter iteration results into a list.\n\n```sema\n(for/filter ((x (range 10)))\n  (even? x))  ; => (0 2 4 6 8)\n```"),
        ("for/fold", "Fold over iteration with an accumulator.\n\n```sema\n(for/fold ((sum 0))\n  ((x (range 5)))\n  (+ sum x))  ; => 10\n```"),
        ("with-budget", "Limit LLM token budget for enclosed operations.\n\n```sema\n(with-budget 1000\n  (prompt \"Be brief.\"))\n```"),
        ("def", "Alias for `define`. Define a variable.\n\n```sema\n(def x 42)\n```"),
    ];

    for (name, doc) in special_forms {
        docs.insert(name.to_string(), doc.to_string());
    }

    docs
}
