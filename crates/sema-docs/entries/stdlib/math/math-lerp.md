---
name: "math/lerp"
module: "math"
section: "Interpolation & Clamping"
params: [{ name: a, type: number }, { name: b, type: number }, { name: t, type: number }]
returns: "number"
see_also: ["math/map-range", "math/clamp"]
---

Linear interpolation between two values. `(math/lerp a b t)` returns `a + (b - a) * t`, so `t=0` gives `a` and `t=1` gives `b`. `t` is not clamped — values outside `[0, 1]` extrapolate past the endpoints.

```sema
(math/lerp 0 100 0.5)   ; => 50.0
(math/lerp 0 100 0.25)  ; => 25.0
(math/lerp 10 20 0.0)   ; => 10.0
(math/lerp 0 100 1.5)   ; => 150.0   ; t > 1 extrapolates beyond b
```

To remap a value from one range to another instead of supplying `t` directly, use [`math/map-range`](#math-map-range).
