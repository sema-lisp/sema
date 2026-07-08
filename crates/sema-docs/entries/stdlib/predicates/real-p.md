---
name: "real?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["complex?", "rational?", "exact?", "exact-integer?", "inexact?"]
---

Test whether a number is real — has no non-zero imaginary part. Every integer, rational, and float is real; `real?` is false *only* for a complex number with a genuine imaginary component. A complex written with an exact-zero imaginary part collapses to a real, so `3+0i` is real.

```sema
(real? 42)      ; => #t
(real? 3.14)    ; => #t
(real? 1/3)     ; => #t
(real? 3+4i)    ; => #f   ; non-zero imaginary part
(real? 3+0i)    ; => #t   ; imaginary part is exact zero
```

[`complex?`](../complex-p) is the wider test (true for all numbers);
[`rational?`](../rational-p) is narrower (excludes floats).
