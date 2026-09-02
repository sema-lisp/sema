---
name: "bit/and"
module: "math"
section: "Bitwise Operations"
params: [{ name: a, type: int }, { name: b, type: int }]
returns: "int"
see_also: ["bit/or", "bit/xor", "bit/not"]
---

Bitwise AND (1 only where both operands have a 1 bit). Commonly used as a mask to keep just the low bits or test a flag: `(bit/and x 255)` isolates the low 8 bits.

```sema
(bit/and 5 3)        ; => 1
(bit/and 15 9)       ; => 9
(bit/and 274 255)    ; => 18   (mask to the low 8 bits)
```
