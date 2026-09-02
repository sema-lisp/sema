---
name: "abs"
module: "math"
section: "Numeric Utilities"
params: [{ name: n, type: number }]
returns: "number"
see_also: ["math/sign", "math/clamp", "magnitude"]
---

Absolute value.

```sema
(abs -5)      ; => 5
(abs 3)       ; => 3
(abs -3.14)   ; => 3.14
```
