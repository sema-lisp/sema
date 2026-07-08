---
name: "math/remainder"
module: "math"
section: "Integer Math"
params: [{ name: n, type: integer }, { name: d, type: integer }]
returns: "integer"
see_also: ["remainder", "math/quotient", "modulo"]
---

Remainder of truncated integer division: `n - d * (quotient n d)`, so the result takes the sign of the *dividend* `n`, per R7RS. Bignum-aware; both arguments must be exact integers, and a zero divisor raises an error. This is the slash-namespaced spelling of [`remainder`](../remainder) (same implementation, identical behavior); pairs with [`math/quotient`](../math-quotient) so that `(+ (* (math/quotient n d) d) (math/remainder n d))` reconstructs `n`.

```sema
(math/remainder 10 3)  ; => 1
(math/remainder -7 2)  ; => -1   ; sign of the dividend
(math/remainder 7 -2)  ; => 1    ; still follows the dividend
```

Unlike [`modulo`](../modulo), whose result follows the divisor's sign,
`math/remainder` follows the dividend.
