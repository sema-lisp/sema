---
name: "defun"
module: "special-forms"
syntax: "(defun name (params ...) body ...)"
---

Define a named function. `defun` is syntactic sugar that expands into `(define (name params ...) body ...)`. It takes a function name symbol, a parameter list, and one or more body expressions. The last body expression is evaluated in tail position.

Parameter lists support rest arguments via dot notation: `(defun f (x . rest) rest)`. Destructuring patterns are also supported in parameter positions. `defun` always returns `nil`.

The alias `defn` is accepted as an alternative spelling (Clojure-style). Both forms are handled identically by the evaluator.

```sema
(defun greet (name)
  (string-append "Hello, " name))
(greet "world")  ; => "Hello, world"
```

```sema
(defun sum-list (xs)
  (if (null? xs)
      0
      (+ (car xs) (sum-list (cdr xs)))))
(sum-list '(1 2 3))  ; => 6
```

```sema
(defun greet-many (greeting . names)
  (map (fn (n) (string-append greeting " " n)) names))
(greet-many "Hi" "Ada" "Bob")  ; => ("Hi Ada" "Hi Bob")
```
