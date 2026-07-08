---
name: "denominator"
module: "math"
section: "Rational Accessors"
params: [{ name: x, type: number }]
returns: "integer"
see_also: ["numerator"]
---

Return the denominator of an exact rational number, always stored in lowest terms. An integer `n` counts as `n/1`, so its denominator is `1`. Applying it to a float or a complex number raises a type error (`expected rational, got float`) — floats have no canonical rational decomposition here, so convert with [`exact`](../exact) first if that is what you want.

```sema
(denominator 22/7)    ; => 7
(denominator 1/2)     ; => 2
(denominator -6/4)    ; => 2    ; reduced to -3/2 first
(denominator 42)      ; => 1    ; integers are n/1
(denominator 0)       ; => 1
```

Pairs with [`numerator`](#numerator): `(/ (numerator x) (denominator x))`
reconstructs the reduced fraction.
