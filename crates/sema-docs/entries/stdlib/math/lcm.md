---
name: "lcm"
module: "math"
section: "Integer Math"
params: [{ name: ns, type: integer, variadic: true }]
returns: "integer"
see_also: ["gcd", "math/lcm"]
---

Least common multiple: the smallest non-negative integer that every argument divides evenly. Variadic and bignum-aware, and it ignores the sign of its arguments (the result is always non-negative). If any argument is `0` the result is `0`. `(lcm)` with no arguments is `1` — the identity of the fold — per R7RS. Pairs with [`gcd`](#gcd); [`math/lcm`](../math-lcm) is the namespaced alias.

```sema
(lcm 4 6)      ; => 12
(lcm 2 3 4)    ; => 12
(lcm -4 6)     ; => 12   ; sign-independent
(lcm 0 5)      ; => 0    ; zero absorbs
(lcm 7 13)     ; => 91   ; coprime
(lcm)          ; => 1
```
