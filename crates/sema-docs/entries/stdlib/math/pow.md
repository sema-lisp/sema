---
name: "pow"
module: "math"
section: "Numeric Utilities"
params: [{ name: base, type: number }, { name: exponent, type: number }]
returns: "number"
see_also: ["expt", "math/pow", "sqrt"]
---

Raise a number to a power. Exact integer operands stay exact: a non-negative exponent gives an exact integer result, and a negative exponent gives the exact reciprocal as a rational — it does not fall back to a float. A fractional exponent (or float operands) yields a float. `expt` is the Scheme-style alias, with identical semantics.

```sema
(pow 2 10)    ; => 1024
(pow 3 3)     ; => 27
(pow 2 -1)    ; => 1/2   ; negative exponent -> exact rational, not a float
(pow 9 0.5)   ; => 3.0   ; fractional exponent -> square root
```
