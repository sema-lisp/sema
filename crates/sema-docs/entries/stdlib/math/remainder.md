---
name: "remainder"
module: "math"
section: "Integer Math"
params: [{ name: n, type: integer }, { name: d, type: integer }]
returns: "integer"
see_also: ["quotient", "modulo", "math/remainder"]
---

Remainder of truncated integer division: `n - d * (quotient n d)`, so the result takes the sign of the *dividend* `n`, per R7RS. Bignum-aware; both arguments must be exact integers, and a zero divisor raises an error (`division by zero`). Pairs with [`quotient`](#quotient) so that `(+ (* (quotient n d) d) (remainder n d))` reconstructs `n`.

```sema
(remainder 10 3)   ; => 1
(remainder -7 2)   ; => -1   ; sign of the dividend
(remainder 7 -2)   ; => 1    ; still follows the dividend
```

Unlike [`modulo`](#modulo), whose result follows the *divisor*'s sign,
`remainder` follows the dividend — so `(remainder -7 2)` is `-1` where
`(modulo -7 2)` is `1`.
