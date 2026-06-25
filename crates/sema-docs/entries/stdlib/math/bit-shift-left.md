---
name: "bit/shift-left"
module: "math"
section: "Bitwise Operations"
params: [{ name: n, type: int }, { name: count, type: int, doc: "shift amount, 0..64" }]
returns: "int"
---

Left bit shift.

```sema
(bit/shift-left 1 4)   ; => 16
(bit/shift-left 3 2)   ; => 12
```
