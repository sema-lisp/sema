---
name: "math/infinite?"
module: "math"
section: "Numeric Predicates"
params: [{ name: x, type: any }]
returns: "bool"
see_also: ["math/nan?", "math/infinity"]
---

Test if a value is infinite.

```sema
(math/infinite? math/infinity)  ; => #t
(math/infinite? 42)             ; => #f
```
