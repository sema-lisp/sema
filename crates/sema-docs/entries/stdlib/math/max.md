---
name: "max"
module: "math"
section: "Numeric Utilities"
syntax: "(max num ...)"
returns: "number"
see_also: ["min", "math/clamp"]
---

Return the largest of 1 or more numbers (the no-arg case errors). Mixed integers and floats compare by value; the chosen argument keeps its own type.

```sema
(max 1 2 3)   ;; => 3
(max 1 2.5)   ;; => 2.5   ; the larger value wins, as a float
(max 5)       ;; => 5
(max)         ;; error: Arity error: max expects 1+ args, got 0
```

To take the max of a list, splat it: `(apply max '(4 9 2))` ;; => 9.
