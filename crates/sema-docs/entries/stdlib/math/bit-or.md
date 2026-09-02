---
name: "bit/or"
module: "math"
section: "Bitwise Operations"
params: [{ name: a, type: int }, { name: b, type: int }]
returns: "int"
see_also: ["bit/and", "bit/xor", "bit/not"]
---

Bitwise OR.

```sema
(bit/or 5 3)       ; => 7
(bit/or 8 4)       ; => 12
```
