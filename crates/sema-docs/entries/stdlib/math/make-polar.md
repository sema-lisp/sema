---
name: "make-polar"
module: "math"
section: "Complex Construction"
params: [{ name: magnitude, type: number }, { name: angle, type: number }]
returns: "number"
see_also: ["make-rectangular", "magnitude", "angle", "real-part", "imag-part"]
---

Construct a complex number from polar coordinates: magnitude `r` and angle `θ` in radians, giving `r·cos θ + r·sin θ·i`. The conversion runs through `cos`/`sin` in floating point, so the result is always an inexact complex number (both parts are floats) — even when the angle is `0`.

```sema
(make-polar 2 0)                  ; => 2.0+0.0i
(make-polar 5 (math/atan2 3 4))   ; => 4.0+3.0i   ; magnitude 5 at atan2(3,4)
(make-polar 1 pi)                 ; => -1.0+0.00000000000000012246467991473532i
```

The tiny `1.22e-16` imaginary part in the last example is genuine floating-point
dust: `sin(π)` is not exactly `0` in `f64`. Polar conversions carry this kind of
rounding, so compare results with a tolerance rather than for exact equality.
[`make-rectangular`](../make-rectangular) builds a complex number from real and
imaginary parts instead, and [`magnitude`](../magnitude)/[`angle`](../angle) recover the polar coordinates.
