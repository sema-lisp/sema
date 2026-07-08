---
name: "math/lcm"
module: "math"
section: "Integer Math"
params: [{ name: ns, type: integer, variadic: true }]
returns: "integer"
see_also: ["lcm", "math/gcd"]
---

Least common multiple: the smallest non-negative integer that every argument divides evenly. Variadic and bignum-aware, sign-independent, and `0` whenever any argument is `0`. `(math/lcm)` with no arguments is `1` — the identity of the fold — per R7RS. This is the slash-namespaced spelling of [`lcm`](../lcm) (same implementation, identical behavior); pairs with [`math/gcd`](../math-gcd).

```sema
(math/lcm 4 6)     ; => 12
(math/lcm 2 3 4)   ; => 12
(math/lcm -4 6)    ; => 12   ; sign-independent
(math/lcm 0 5)     ; => 0    ; zero absorbs
(math/lcm 7 13)    ; => 91   ; coprime
(math/lcm)         ; => 1
```
