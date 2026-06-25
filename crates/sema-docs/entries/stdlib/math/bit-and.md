---
name: "bit/and"
module: "math"
section: "Bitwise Operations"
params: [{ name: a, type: int }, { name: b, type: int }]
returns: "int"
---

Bitwise AND.

```sema
(bit/and 5 3)      ; => 1
(bit/and 15 9)     ; => 9
```
