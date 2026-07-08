---
name: "math/gcd"
module: "math"
section: "Integer Math"
params: [{ name: ns, type: integer, variadic: true }]
returns: "integer"
see_also: ["gcd", "math/lcm"]
---

Greatest common divisor: the largest non-negative integer that divides every argument. Variadic and bignum-aware, and it ignores the sign of its arguments (the result is always non-negative). `(math/gcd)` with no arguments is `0` — the identity of the fold — per R7RS. This is the slash-namespaced spelling of [`gcd`](../gcd) (same implementation, identical behavior); pairs with [`math/lcm`](../math-lcm).

```sema
(math/gcd 12 8)     ; => 4
(math/gcd 15 10 25) ; => 5
(math/gcd -12 8)    ; => 4    ; sign-independent
(math/gcd 7 13)     ; => 1    ; coprime
(math/gcd)          ; => 0
```
