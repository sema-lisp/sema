---
name: "math/lerp"
module: "math"
section: "Interpolation & Clamping"
---

Linear interpolation between two values. `(math/lerp a b t)` returns `a + (b - a) * t`.

```sema
(math/lerp 0 100 0.5)   ; => 50.0
(math/lerp 0 100 0.25)  ; => 25.0
(math/lerp 10 20 0.0)   ; => 10.0
```
