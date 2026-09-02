---
name: "bit/xor"
module: "math"
section: "Bitwise Operations"
params: [{ name: a, type: int }, { name: b, type: int }]
returns: "int"
see_also: ["bit/and", "bit/or", "bit/not"]
---

Bitwise XOR (set bits where the operands differ). Self-inverse: `(bit/xor x x)` is `0` and `(bit/xor x 0)` is `x`, which makes it the standard tool for toggling flags.

```sema
(bit/xor 5 3)      ; => 6
(bit/xor 5 5)      ; => 0   (anything XOR itself is 0)
(bit/xor 5 0)      ; => 5   (XOR with 0 is identity)
```
