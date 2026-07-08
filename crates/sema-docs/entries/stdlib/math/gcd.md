---
name: "gcd"
module: "math"
section: "Integer Math"
params: [{ name: ns, type: integer, variadic: true }]
returns: "integer"
see_also: ["lcm", "math/gcd"]
---

Greatest common divisor: the largest non-negative integer that divides every argument. Variadic and bignum-aware, and it ignores the sign of its arguments (the result is always non-negative). `(gcd)` with no arguments is `0` — the identity of the fold — per R7RS, and `(gcd n)` is `|n|`. Pairs with [`lcm`](#lcm); [`math/gcd`](../math-gcd) is the namespaced alias.

```sema
(gcd 12 8)     ; => 4
(gcd 15 10 25) ; => 5
(gcd -12 8)    ; => 4    ; sign-independent
(gcd 7 13)     ; => 1    ; coprime
(gcd)          ; => 0
```
