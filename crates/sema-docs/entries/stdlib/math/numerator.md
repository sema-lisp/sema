---
name: "numerator"
module: "math"
section: "Rational Accessors"
params: [{ name: x, type: number }]
returns: "integer"
see_also: ["denominator"]
---

Return the numerator of an exact rational number, taken in lowest terms (so the sign lives on the numerator). An integer `n` is `n/1`, so its numerator is `n` itself. Applying it to a float or a complex number raises a type error (`expected rational, got float`) — convert with [`exact`](../exact) first if you need a rational.

```sema
(numerator 22/7)   ; => 22
(numerator 1/2)    ; => 1
(numerator -6/4)   ; => -3   ; reduced to -3/2 first
(numerator 42)     ; => 42   ; integers are n/1
```

Pairs with [`denominator`](#denominator): `(/ (numerator x) (denominator x))`
reconstructs the reduced fraction.
