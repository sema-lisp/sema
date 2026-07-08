---
name: "exact-integer?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["exact?", "rational?", "real?", "complex?", "inexact?"]
---

Test whether a value is an exact integer — true exactly when both `exact?` and `integer?` hold. This is stricter than a bare `integer?`: `2.0` is an integer *value* but an inexact one, so it fails `exact-integer?`. A rational like `1/2` fails because it is not an integer, while an exact integer wrapped as `3+0i` still counts (its exact-zero imaginary part collapses to a real integer).

```sema
(exact-integer? 42)    ; => #t
(exact-integer? -17)   ; => #t
(exact-integer? 1/2)   ; => #f   ; not an integer
(exact-integer? 2.0)   ; => #f   ; inexact
(exact-integer? 3+0i)  ; => #t   ; exact integer with exact-zero imaginary
```

Use [`exact?`](../exact-p) alone to also admit exact rationals.
