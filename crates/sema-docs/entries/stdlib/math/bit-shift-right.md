---
name: "bit/shift-right"
module: "math"
section: "Bitwise Operations"
params: [{ name: n, type: int }, { name: count, type: int }]
returns: "int"
---

Right bit shift.

```sema
(bit/shift-right 16 2) ; => 4
(bit/shift-right 8 1)  ; => 4
```
