---
name: "math/atan2"
module: "math"
section: "Trigonometry"
params: [{ name: y, type: number }, { name: x, type: number }]
returns: "float"
see_also: ["math/atan", "math/tan", "angle"]
---

Two-argument inverse tangent: `(math/atan2 y x)`. Returns the angle in radians
(in `-π..π`) between the positive x-axis and the point `(x, y)`. The argument
order is **y first, then x** (matching C and most languages), which lets it pick
the correct quadrant where plain `math/atan` (seeing only `y/x`) cannot.

```sema
(math/atan2 1 1)   ; => ~0.7854   (π/4, point in quadrant I)
(math/atan2 1 0)   ; => ~1.5708   (π/2, straight up the y-axis)
(math/atan2 0 -1)  ; => ~3.1416   (π,  along the negative x-axis)
```
