---
name: "let*-values"
module: "special-forms"
syntax: "(let*-values (((name ...) producer) ...) body ...)"
---

Like `let-values`, but binding is **sequential**: each clause's `producer` is evaluated in an environment that already contains every earlier clause's bindings, so a later clause can refer to an earlier one's names. Formals follow the same rules as `let-values` — `(a b)`, dotted `(a . rest)`, or a bare symbol binding all values as a list.

```sema
(define a 100)
(let*-values (((a) (values 1))
              ((b) (values a)))   ; sees the NEW a (1) from the clause just above
  b)
;; => 1

(let*-values (((a b) (values 1 2))
              ((c) (values (+ a b))))
  c)
;; => 3
```

**See also:** `let-values` (parallel — later producers can't see earlier bindings), `call-with-values`, `define-values`.
