---
name: "round"
module: "math"
section: "Numeric Utilities"
params: [{ name: n, type: number }]
returns: "number"
---

Round to the nearest integer, with ties (`.5`) rounded to the nearest *even* integer (banker's rounding, per R7RS) rather than away from zero. `round` is also exactness-preserving: a float input rounds to a float, an exact rational rounds to an exact integer. For decimal-place rounding use [`math/round-to`](#math-round-to); to drop the fraction toward zero use [`truncate`](#truncate).

```sema
(round 3.5)   ; => 4.0
(round 3.4)   ; => 3.0
(round 2.5)   ; => 2.0   ; tie rounds to even, not away from zero
(round -2.5)  ; => -2.0  ; tie rounds to even
(round 7/2)   ; => 4     ; exact rational -> exact integer (tie to even)
```
