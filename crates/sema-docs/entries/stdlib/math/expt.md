---
name: "expt"
module: "math"
section: "Scheme Aliases"
params: [{ name: base, type: number }, { name: exp, type: number }]
returns: "number"
---

Raise a base to a power, `(expt base exponent)`. The Scheme name for `pow`. An exact integer base with an exact integer exponent stays exact: a non-negative exponent gives an exact integer (bignum if it overflows a machine word), and a negative exponent gives the exact reciprocal as a rational. A fractional exponent gives a float (so `(expt x 0.5)` is a square root).

```sema
(expt 2 10)    ; => 1024
(expt 2 100)   ; => 1267650600228229401496703205376   (exact bignum)
(expt 9 0.5)   ; => 3.0     (square root)
(expt 2 -1)    ; => 1/2     (exact reciprocal, not a float)
```
