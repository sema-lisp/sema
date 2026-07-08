---
name: "ceil"
module: "math"
section: "Numeric Utilities"
---

Round up toward positive infinity. For negatives this rounds *toward* zero (`-2.7` → `-2`). The mirror of `floor`; alias `ceiling`. Exactness-preserving: a float input rounds to a float, an exact rational rounds to an exact integer.

```sema
(ceil 3.2)    ; => 4.0
(ceil -2.7)   ; => -2.0
(ceil 7/2)    ; => 4     (exact rational -> exact integer)
(ceil 5)      ; => 5     (already an integer, unchanged)
```
