---
name: "for"
module: "special-forms"
syntax: "(for ((var expr) ...) body ...)"
---

Iterate over one or more sequences for side effects. Each binding pairs a variable with an expression that evaluates to a sequence (list, vector, or range). The body is evaluated once for every combination of elements across all sequences, with the variables bound in a fresh lexical scope. `for` returns `nil`.

Multiple bindings produce nested iteration (cartesian product), with the rightmost binding changing fastest. This form is similar to `for-each` but uses inline `let`-style bindings rather than a separate lambda. It is commonly used for printing, mutating external state, or performing I/O over collections.

```sema
(for ((x (range 5)))
  (println x))
;; prints 0 through 4
```

```sema
(for ((x (list 1 2 3))
      (y (list 10 20)))
  (println (list x y)))
;; prints (1 10), (1 20), (2 10), (2 20), (3 10), (3 20)
```

```sema
(let ((total 0))
  (for ((n (list 10 20 30)))
    (set! total (+ total n)))
  total)
;; => 60
```

**Note:** `for` is typically provided as a macro that expands into `map` and sequence combinators. The variables are locally scoped and are not visible outside the form.