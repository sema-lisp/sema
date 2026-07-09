---
name: "let-values"
module: "special-forms"
syntax: "(let-values (((name ...) producer) ...) body ...)"
---

Bind the values produced by one or more `values`-producing expressions to local names, then evaluate `body`. Like `let`, binding is **parallel**: every `producer` is evaluated against the *outer* environment before any clause's names come into scope, so a later clause cannot see an earlier clause's bindings.

Each clause's formals can be:

- `(a b)` — bind exactly that many values by name.
- `(a . rest)` — bind the first values by name, remaining values as a list in `rest` (dotted formals, same shape as a lambda's rest parameter).
- `all` (a bare symbol, no parens) — bind ALL produced values as a single list.

A producer that isn't a call to `values` is treated as a single value.

```sema
(let-values (((a b) (values 1 2)))
  (+ a b))
;; => 3

(let-values (((a . rest) (values 1 2 3)))
  rest)
;; => (2 3)

(let-values ((all (values 1 2 3)))
  all)
;; => (1 2 3)
```

Multiple clauses evaluate independently against the outer environment:

```sema
(define a 100)
(let-values (((a) (values 1))
             ((b) (values a)))   ; sees the OUTER a (100), not the new binding above
  b)
;; => 100
```

If a clause produces the wrong number of values for its formals, that's an arity error (R7RS "wrong number of values"). A producer's error propagates normally.

**See also:** `let*-values` (sequential — later producers see earlier bindings), `call-with-values` (the lower-level primitive this desugars to), `define-values` (top-level/global version).
