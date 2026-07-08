---
name: "/"
module: "math"
section: "Basic Arithmetic"
syntax: "(/ num num ...)"
returns: "number"
---

Divide numbers. Dividing two exact numbers (integers or rationals) stays exact: an inexact result becomes a reduced exact rational rather than a float, so `(/ 10 3)` is `10/3`, not `3.3333...`. Introducing a float operand makes the result inexact. For truncated integer division use [`math/quotient`](#math-quotient).

```sema
(/ 10 2)      ;; => 5
(/ 10 3)      ;; => 10/3               (exact rational, not a float)
(/ 10.0 3)    ;; => 3.3333333333333335 (a float operand makes it inexact)
```
