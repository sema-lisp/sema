---
name: "floor"
module: "math"
section: "Numeric Utilities"
params: [{ name: n, type: number }]
returns: "number"
---

Round down toward negative infinity. For negatives this rounds *away* from zero (`-2.3` → `-3`), unlike `int`, which truncates toward zero (`-2.3` → `-2`). `floor` is exactness-preserving: a float input rounds to a float, and an exact rational rounds to an exact integer.

```sema
(floor 3.7)   ; => 3.0
(floor -2.3)  ; => -3.0  (down, not -2.0)
(floor 7/2)   ; => 3     (exact rational -> exact integer)
(int -2.3)    ; => -2    (contrast: toward zero)
```
