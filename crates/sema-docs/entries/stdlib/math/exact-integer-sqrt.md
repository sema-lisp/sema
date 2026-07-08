---
name: "exact-integer-sqrt"
module: "math"
section: "Integer Square Root"
params: [{ name: n, type: integer }]
returns: "list"
see_also: ["exact", "quotient"]
---

Compute the exact integer square root of a non-negative integer. Returns a two-element list `(s r)` where `s` is the floor of √n and `r` is the remainder, so that `s*s + r = n` with `0 ≤ r ≤ 2s`. Both values are exact integers and the computation is exact for arbitrarily large bignums (no float rounding). A negative argument raises an error (`argument must be non-negative`).

```sema
(exact-integer-sqrt 17)                ; => (4 1)   ; 4*4 + 1 = 17
(exact-integer-sqrt 100)               ; => (10 0)  ; perfect square
(exact-integer-sqrt 0)                 ; => (0 0)
(exact-integer-sqrt 15241578750190521) ; => (123456789 0)  ; exact on bignums
```
