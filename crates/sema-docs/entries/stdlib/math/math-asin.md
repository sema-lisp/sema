---
name: "math/asin"
module: "math"
section: "Trigonometry"
params: [{ name: x, type: number }]
returns: "float"
see_also: ["sin", "math/acos", "math/atan"]
---

Inverse sine. Returns radians.

```sema
(math/asin 1)      ; => ~1.5707 (π/2)
(math/asin 0)      ; => 0.0
```
