---
name: "math/map-range"
module: "math"
section: "Interpolation & Clamping"
params: [{ name: value, type: number }, { name: in-min, type: number }, { name: in-max, type: number }, { name: out-min, type: number }, { name: out-max, type: number }]
returns: "number"
see_also: ["math/lerp", "math/clamp"]
---

Linearly rescale `value` from the input range `[in-min, in-max]` to the output range `[out-min, out-max]`, preserving its relative position. `(math/map-range value in-min in-max out-min out-max)`. Like [`math/lerp`](#math-lerp), it does not clamp — inputs outside the source range map outside the target range.

```sema
(math/map-range 5 0 10 0 100)    ; => 50.0
(math/map-range 0.5 0 1 0 255)   ; => 127.5
(math/map-range 20 0 10 0 100)   ; => 200.0   ; over-range input maps past out-max
```

Ranges may also invert: `(math/map-range 0 0 10 100 0)` => 100.0 maps a low input to a high output.
