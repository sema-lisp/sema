---
name: "bit/shift-right"
module: "math"
section: "Bitwise Operations"
params: [{ name: n, type: int }, { name: count, type: int }]
returns: "int"
see_also: ["bit/shift-left", "bit/and", "bit/or"]
---

Right bit shift: `(bit/shift-right n k)` shifts `n`'s bits right by `k`, discarding the low `k` bits — equivalent to integer division by `2^k` (rounding toward negative infinity). The inverse of `bit/shift-left`.

```sema
(bit/shift-right 16 2) ; => 4   (16 / 2^2)
(bit/shift-right 8 1)  ; => 4   (8 / 2^1)
(bit/shift-right 7 1)  ; => 3   (low bit dropped)
```
