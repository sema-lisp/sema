---
name: "math/map-range"
module: "math"
section: "Interpolation & Clamping"
---

Map a value from one range to another. `(math/map-range value in-min in-max out-min out-max)`.

```sema
(math/map-range 5 0 10 0 100)    ; => 50.0
(math/map-range 0.5 0 1 0 255)   ; => 127.5
```
