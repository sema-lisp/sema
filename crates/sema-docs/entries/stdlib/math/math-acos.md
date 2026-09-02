---
name: "math/acos"
module: "math"
section: "Trigonometry"
params: [{ name: x, type: number }]
returns: "float"
see_also: ["cos", "math/asin", "math/atan"]
---

Inverse cosine. Returns radians.

```sema
(math/acos 0)      ; => ~1.5707 (π/2)
(math/acos 1)      ; => 0.0
```
