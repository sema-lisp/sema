---
name: "define-values"
module: "special-forms"
syntax: "(define-values (name ...) producer)"
---

Bind the values produced by `producer` to top-level (or, inside a body, local) definitions — the `define` analogue of `let-values`. Formals follow the same rules: `(a b)` for exact names, dotted `(a . rest)` for a fixed prefix plus the remaining values as a list.

```sema
(define-values (a b) (values 10 20))
(+ a b)
;; => 30

(define-values (q . r) (values 1 2 3))
r
;; => (2 3)
```

A producer that isn't a call to `values` is treated as a single value, same as `let-values`/`call-with-values`.

**See also:** `let-values`/`let*-values` (scoped binding versions), `call-with-values`, `values`.
