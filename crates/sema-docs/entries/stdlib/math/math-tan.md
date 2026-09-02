---
name: "math/tan"
module: "math"
section: "Trigonometry"
params: [{ name: x, type: number, doc: "angle in radians" }]
returns: "float"
see_also: ["sin", "cos", "math/atan"]
---

Tangent (argument in radians).

```sema
(math/tan 0)       ; => 0.0
(math/tan (/ pi 4)); => ~1.0
```
