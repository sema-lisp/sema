---
name: "call-with-values"
module: "lists"
section: "Multiple Values"
syntax: "(call-with-values producer consumer)"
returns: "any"
---

Call the zero-argument `producer` thunk, then apply `consumer` to whatever it produced. If `producer` returned a bundle from `values` (zero or two-or-more values), each becomes a separate argument to `consumer`; an ordinary single value (including a plain, non-`values` return) is passed as `consumer`'s one argument.

```sema
(call-with-values (lambda () (values 1 2)) +)          ; => 3
(call-with-values (lambda () (values 1 2 3)) list)      ; => (1 2 3)
(call-with-values (lambda () 42) list)                  ; => (42)   ; single value, not spread
(call-with-values (lambda () (values)) (lambda () 99))  ; => 99     ; zero values, zero-arg consumer
```

If the number of produced values doesn't match `consumer`'s arity, the call fails with the ordinary lambda-arity error (R7RS's "wrong number of values"):

```sema
(call-with-values (lambda () (values 1 2)) (lambda (x) x))
;; Arity error: <lambda> expects 1 args, got 2
```

An error thrown by `producer` (or by `consumer`) propagates normally — `call-with-values` does not catch or swallow it.

`let-values` and `let*-values` are the binding-form sugar built on top of `call-with-values`; reach for those directly when you just want local names bound to the produced values.

**Note:** because `producer`/`consumer` are invoked through the same native dispatch as `apply`, a call across this boundary is not a true VM tail call — deep recursion written through `call-with-values` won't get the same tail-call optimization as a plain named-let.

**See also:** `values` (produce the values), `apply` (spread a list's elements as arguments), `let-values`/`let*-values` (binding-form sugar).
