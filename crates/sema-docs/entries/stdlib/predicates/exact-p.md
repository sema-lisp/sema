---
name: "exact?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["inexact?", "exact-integer?", "rational?", "real?", "complex?"]
---

Test whether a number is exact — represented without floating point. Exact numbers are integers, exact rationals, and complex numbers whose real *and* imaginary parts are both exact. Any float, or any complex value with an inexact component, is inexact and returns `#f`. Exactly the complement of [`inexact?`](../inexact-p) on numbers.

```sema
(exact? 42)      ; => #t
(exact? 1/3)     ; => #t
(exact? 3.14)    ; => #f   ; float
(exact? 3+4i)    ; => #t   ; both components exact
(exact? 3.0+4i)  ; => #f   ; real component inexact
```

Convert between the two forms with [`exact`](../../math/exact) and
[`inexact`](../../math/inexact).
