---
name: "math/log2"
module: "math"
section: "Exponential & Logarithmic"
params: [{ name: x, type: number }]
returns: "float"
see_also: ["log", "math/log10"]
---

Base-2 logarithm.

```sema
(math/log2 8)      ; => 3.0
(math/log2 1024)   ; => 10.0
```
