---
name: "rational?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["real?", "exact?", "exact-integer?", "complex?", "inexact?"]
---

Test whether a number is rational — exact and expressible as a ratio of two integers. Every exact integer and every exact rational qualifies. Floats return `#f` (they are inexact), and complex numbers with a non-zero imaginary part return `#f` (they are not real). In effect this predicate coincides with [`exact?`](../exact-p) restricted to real numbers.

```sema
(rational? 42)     ; => #t
(rational? 1/3)    ; => #t
(rational? 3.14)   ; => #f   ; inexact
(rational? 3+4i)   ; => #f   ; not real
```

Note this is stricter than strict R7RS, where a finite float is also rational;
here rationality tracks exactness. For any real number regardless of exactness,
use [`real?`](../real-p).
