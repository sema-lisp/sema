---
name: "min"
module: "math"
section: "Numeric Utilities"
syntax: "(min num ...)"
returns: "number"
---

Return the smallest of 1 or more numbers (the no-arg case errors).

```sema
(min 1 2 3)   ;; => 1
(min 5)       ;; => 5
(min)         ;; error: Arity error: min expects 1+ args, got 0
```
