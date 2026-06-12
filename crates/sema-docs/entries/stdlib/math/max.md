---
name: "max"
module: "math"
section: "Numeric Utilities"
---

Return the largest of 1 or more numbers (the no-arg case errors).

```sema
(max 1 2 3)   ;; => 3
(max 5)       ;; => 5
(max)         ;; error: Arity error: max expects 1+ args, got 0
```
