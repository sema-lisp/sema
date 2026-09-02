---
name: "min"
module: "math"
section: "Numeric Utilities"
syntax: "(min num ...)"
returns: "number"
see_also: ["max", "math/clamp"]
---

Return the smallest of 1 or more numbers (the no-arg case errors). Mixed integers and floats compare by value; the chosen argument keeps its own type.

```sema
(min 1 2 3)     ;; => 1
(min 3 1.5 2)   ;; => 1.5
(min 5)         ;; => 5
(min)           ;; error: Arity error: min expects 1+ args, got 0
```

To take the min of a list, splat it: `(apply min '(4 9 2))` ;; => 2.
