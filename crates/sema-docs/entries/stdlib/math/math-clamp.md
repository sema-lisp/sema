---
name: "math/clamp"
module: "math"
section: "Interpolation & Clamping"
params: [{ name: x, type: number }, { name: lo, type: number }, { name: hi, type: number }]
returns: "number"
---

Clamp a value to a range.

```sema
(math/clamp 15 0 10)   ; => 10
(math/clamp -5 0 10)   ; => 0
(math/clamp 5 0 10)    ; => 5
```
