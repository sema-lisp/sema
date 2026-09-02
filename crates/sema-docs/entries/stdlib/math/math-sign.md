---
name: "math/sign"
module: "math"
section: "Interpolation & Clamping"
params: [{ name: n, type: number }]
returns: "number"
see_also: ["abs", "math/clamp"]
---

Return the sign of a number: -1, 0, or 1.

```sema
(math/sign -5)     ; => -1
(math/sign 0)      ; => 0
(math/sign 42)     ; => 1
```
