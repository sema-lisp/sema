---
name: "math/exp"
module: "math"
section: "Exponential & Logarithmic"
params: [{ name: x, type: number }]
returns: "float"
see_also: ["log", "e", "pow"]
---

Euler's number raised to a power (e^x).

```sema
(math/exp 1)       ; => 2.71828...
(math/exp 0)       ; => 1.0
```
