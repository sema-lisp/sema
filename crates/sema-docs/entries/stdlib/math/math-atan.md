---
name: "math/atan"
module: "math"
section: "Trigonometry"
params: [{ name: x, type: number }]
returns: "float"
see_also: ["math/atan2", "math/tan", "math/asin"]
---

Inverse tangent. Returns radians.

```sema
(math/atan 1)      ; => ~0.7854 (π/4)
(math/atan 0)      ; => 0.0
```
