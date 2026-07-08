---
name: "inexact?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["exact?", "exact-integer?", "rational?", "real?", "complex?"]
---

Test whether a number is inexact — carries a floating-point component and so may hold rounding error. True for any float and for any complex number with at least one inexact part. Exactly the complement of [`exact?`](../exact-p) on numbers.

```sema
(inexact? 42)      ; => #f   ; exact integer
(inexact? 3.14)    ; => #t
(inexact? 1/3)     ; => #f   ; exact rational
(inexact? 3.0+4i)  ; => #t   ; real component inexact
```

Convert between the two forms with [`inexact`](../../math/inexact) and
[`exact`](../../math/exact).
