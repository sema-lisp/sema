use std::collections::HashMap;

/// Parse a stdlib markdown doc file and extract function name → documentation.
/// Format: ### `name` \n\n description paragraph \n\n ```sema ... ```
fn parse_stdlib_md(md: &str, out: &mut HashMap<String, String>) {
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        // Look for ### `name`
        if let Some(rest) = lines[i].strip_prefix("### `") {
            if let Some(name) = rest.strip_suffix('`') {
                let name = name.to_string();
                i += 1;
                // Skip blank lines
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                // Collect description + example until next heading or end
                let mut doc = String::new();
                while i < lines.len() {
                    if lines[i].starts_with("### ") || lines[i].starts_with("## ") {
                        break;
                    }
                    doc.push_str(lines[i]);
                    doc.push('\n');
                    i += 1;
                }
                let doc = doc.trim_end().to_string();
                if !doc.is_empty() {
                    out.insert(name, doc);
                }
                continue;
            }
        }
        i += 1;
    }
}

/// Build the complete builtin documentation map from embedded stdlib docs.
pub fn build_builtin_docs() -> HashMap<String, String> {
    let mut docs = HashMap::new();

    let sources: &[&str] = &[
        include_str!("../../../website/docs/stdlib/math.md"),
        include_str!("../../../website/docs/stdlib/strings.md"),
        include_str!("../../../website/docs/stdlib/lists.md"),
        include_str!("../../../website/docs/stdlib/maps.md"),
        include_str!("../../../website/docs/stdlib/vectors.md"),
        include_str!("../../../website/docs/stdlib/predicates.md"),
        include_str!("../../../website/docs/stdlib/text-processing.md"),
        include_str!("../../../website/docs/stdlib/file-io.md"),
        include_str!("../../../website/docs/stdlib/system.md"),
        include_str!("../../../website/docs/stdlib/datetime.md"),
        include_str!("../../../website/docs/stdlib/http-json.md"),
        include_str!("../../../website/docs/stdlib/regex.md"),
        include_str!("../../../website/docs/stdlib/csv.md"),
        include_str!("../../../website/docs/stdlib/toml.md"),
        include_str!("../../../website/docs/stdlib/records.md"),
        include_str!("../../../website/docs/stdlib/terminal.md"),
        include_str!("../../../website/docs/stdlib/kv-store.md"),
        include_str!("../../../website/docs/stdlib/bytevectors.md"),
        include_str!("../../../website/docs/stdlib/pdf.md"),
        include_str!("../../../website/docs/stdlib/web-server.md"),
        include_str!("../../../website/docs/stdlib/context.md"),
        include_str!("../../../website/docs/stdlib/playground.md"),
    ];

    for source in sources {
        parse_stdlib_md(source, &mut docs);
    }

    // Special forms documentation (not in stdlib docs)
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
