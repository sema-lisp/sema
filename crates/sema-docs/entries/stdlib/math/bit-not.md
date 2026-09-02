---
name: "bit/not"
module: "math"
section: "Bitwise Operations"
params: [{ name: n, type: int }]
returns: "int"
see_also: ["bit/and", "bit/xor", "bit/or"]
---

Bitwise NOT (one's complement) on the integer's two's-complement representation. Because integers are signed, `(bit/not n)` equals `-n - 1`, so the result of complementing a positive number is negative — this surprises people expecting a small bitmask. Mask with `bit/and` if you want a fixed-width result.

```sema
(bit/not 5)            ; => -6    (= -(5) - 1)
(bit/not 0)            ; => -1
(bit/and (bit/not 5) 255) ; => 250  (low 8 bits only)
```
