---
name: "math/quotient"
module: "math"
section: "Integer Math"
params: [{ name: n, type: integer }, { name: d, type: integer }]
returns: "integer"
see_also: ["quotient", "math/remainder", "modulo"]
---

Truncated integer division: `n ÷ d` rounded toward zero, so the result carries the sign it would have as a real quotient (`-7/2` truncates to `-3`, not floored to `-4`), per R7RS. Bignum-aware; both arguments must be exact integers, and a zero divisor raises an error. This is the slash-namespaced spelling of [`quotient`](../quotient) (same implementation, identical behavior); pairs with [`math/remainder`](../math-remainder) so that `(+ (* (math/quotient n d) d) (math/remainder n d))` reconstructs `n`.

```sema
(math/quotient 10 3)   ; => 3
(math/quotient -7 2)   ; => -3   ; truncates toward zero (not floored to -4)
(math/quotient -7 -2)  ; => 3
(math/quotient 100000000000000000000 7) ; => 14285714285714285714
```
