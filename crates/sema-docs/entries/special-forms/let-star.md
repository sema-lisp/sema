---
name: "let*"
module: "special-forms"
syntax: "(let* ((name value) ...) body ...)"
---

`let*` creates local bindings sequentially. Each init expression is evaluated in an environment that already contains the previous bindings, so later bindings can refer to earlier ones. The result is the value of the last body expression.

Like `let`, `let*` supports destructuring with vector and map patterns. It is useful when a computation builds on values computed just before it, avoiding the need for deeply nested `let` forms.

```sema
(let* ((x 10) (y (* x 2)))
  (+ x y))
;; => 30
```

```sema
(let* (({:keys [name age]} {:name "Ada" :age 36})
       (greeting (format "Hello, ~a" name)))
  greeting)
;; => "Hello, Ada"
```

```sema
(let* ((base 2)
       (exp 8)
       (result (expt base exp)))
  result)
;; => 256
```
