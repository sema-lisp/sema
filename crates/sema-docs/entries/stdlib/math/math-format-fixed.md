---
name: "math/format-fixed"
module: "math"
section: "Rounding"
params: [{ name: n, type: number }, { name: places, type: int, doc: "number of decimal digits" }]
returns: "string"
see_also: ["math/round-to", "number->string"]
---

Format a number as a fixed-decimal **string**, padding trailing zeros to `places` digits — for money/metrics display where `math/round-to` (which returns a float and drops trailing zeros) isn't enough.

```sema
(math/format-fixed 1.2 3)     ; => "1.200"
(math/format-fixed 3.14159 2) ; => "3.14"
```
