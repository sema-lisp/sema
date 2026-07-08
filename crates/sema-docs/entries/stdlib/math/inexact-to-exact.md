---
name: "inexact->exact"
module: "math"
section: "Exactness Conversion"
params: [{ name: x, type: number }]
returns: "number"
see_also: ["exact", "inexact", "exact->inexact"]
---

Convert a number to its exact form. A finite float becomes the exact rational it actually represents (reduced, and normalized to an integer when possible); inexact components of a complex number are converted the same way, and already-exact numbers pass through unchanged. [`exact`](../exact) is the shorter R7RS spelling of this identical operation.

```sema
(inexact->exact 0.5)       ; => 1/2
(inexact->exact 2.0)       ; => 2                              ; normalizes to integer
(inexact->exact 0.1)       ; => 3602879701896397/36028797018963968
(inexact->exact 3.0+4.0i)  ; => 3+4i
```

`0.1` shows the exactness caveat: it has no finite binary expansion, so the
exact value of the stored double is that large power-of-two fraction, not `1/10`.
The inverse direction is [`exact->inexact`](../exact-to-inexact) / [`inexact`](../inexact).
