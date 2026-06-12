---
name: "begin"
module: "special-forms"
syntax: "(begin body ...)"
---

Sequencing special form that evaluates each expression in order and returns the value of the last one. All intermediate results are discarded, so `begin` is primarily used for side effects — printing, mutating state, or performing I/O — grouped into a single expression.

With no arguments, `begin` returns `nil`. The last expression is evaluated in tail position, which means tail-call optimization applies. `progn` is accepted as an alias for `begin` for compatibility with Common Lisp conventions.

```sema
(begin
  (println "step 1")
  (println "step 2")
  (+ 1 2))            ; => 3
```

```sema
(define x 0)
(begin
  (set! x 1)
  (set! x 2)
  x)                  ; => 2
```

```sema
(begin)               ; => nil
```

```sema
(if (> n 0)
  (begin
    (println "positive")
    (process n))
  (begin
    (println "non-positive")
    (skip n)))
```
