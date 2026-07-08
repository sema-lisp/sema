---
name: "sqrt"
module: "math"
section: "Numeric Utilities"
params: [{ name: n, type: number }]
returns: "number"
---

Square root. An exact perfect square returns an exact integer result rather than a float; other non-negative inputs return a float. A negative input returns a complex number rather than `NaN` or raising.

```sema
(sqrt 16)     ; => 4          (exact perfect square, not 4.0)
(sqrt 2)      ; => 1.4142135623730951
(sqrt -1)     ; => 0+1i       (complex result, not NaN)
```
