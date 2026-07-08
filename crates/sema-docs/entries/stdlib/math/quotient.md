---
name: "quotient"
module: "math"
section: "Integer Math"
params: [{ name: n, type: integer }, { name: d, type: integer }]
returns: "integer"
see_also: ["remainder", "modulo", "math/quotient"]
---

Truncated integer division: `n ÷ d` rounded toward zero, so the result carries the sign it would have as a real quotient (`-7/2` truncates to `-3`, not floored to `-4`), per R7RS. Bignum-aware; both arguments must be exact integers, and a zero divisor raises an error (`division by zero`). Pairs with [`remainder`](#remainder) so that `(+ (* (quotient n d) d) (remainder n d))` reconstructs `n`.

```sema
(quotient 10 3)   ; => 3
(quotient -7 2)   ; => -3   ; truncates toward zero (not floored to -4)
(quotient -7 -2)  ; => 3
(quotient 100000000000000000000 7) ; => 14285714285714285714
```

For floored division whose remainder follows the *divisor*'s sign, use
[`modulo`](#modulo) with the ordinary `/`-style quotient instead of `quotient`.
