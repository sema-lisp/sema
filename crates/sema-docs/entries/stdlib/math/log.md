---
name: "log"
module: "math"
section: "Numeric Utilities"
params: [{ name: x, type: number }]
returns: "float"
see_also: ["math/exp", "math/log10", "math/log2"]
---

Natural logarithm (base *e*). Single-argument only — for other bases use `math/log10`, `math/log2`, or divide: `log(x) / log(b)`. Inputs of `0` give `-inf` and negatives give `NaN` rather than erroring.

```sema
(log 1)              ; => 0.0
(log 100)            ; => 4.605170185988092
(/ (log 8) (log 2))  ; => 3.0    (log base 2, via change-of-base)
```
