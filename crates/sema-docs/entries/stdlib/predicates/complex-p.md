---
name: "complex?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["real?", "rational?", "exact?", "inexact?", "exact-integer?"]
---

Test whether a value is a number. In R7RS the number types nest — every real is complex, every rational is real, every integer is rational — so `complex?` is true for *every* number in the tower (integers, rationals, floats, and genuine complex values alike) and false for non-numbers. It is the widest of the numeric predicates.

```sema
(complex? 42)      ; => #t
(complex? 3.14)    ; => #t
(complex? 1/3)     ; => #t
(complex? 3+4i)    ; => #t
(complex? "hi")    ; => #f   ; not a number
```

Narrow the test with [`real?`](../real-p) (excludes non-real complex numbers) or
[`rational?`](../rational-p) (excludes floats and complex numbers).
