---
name: "mod"
module: "math"
section: "Basic Arithmetic"
params: [{ name: a, type: number }, { name: b, type: number }]
returns: "number"
see_also: ["modulo", "remainder", "quotient"]
---

Remainder after **floored** division for exact integers (fixnum or bignum): the result takes the sign of the *divisor*, per R7RS `modulo`. Float operands instead use the IEEE truncated `%` — the result follows the *dividend*'s sign — kept for compatibility with float-heavy code; pass exact integers when you need floored behavior.

```sema
(mod 10 3)     ; => 1
(mod -7 2)     ; => 1     ; floored: sign follows the divisor
(mod 7 -2)     ; => -1
(mod -7.0 2)   ; => -1.0  ; floats: truncated, sign follows the dividend
```

For the truncated remainder on integers (sign follows the dividend), use [`remainder`](#remainder): `(remainder -7 2)` is `-1` where `(mod -7 2)` is `1`.
