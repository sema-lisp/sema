---
name: "/"
module: "math"
section: "Basic Arithmetic"
syntax: "(/ num num ...)"
returns: "number"
---

Divide numbers. Returns a float when the division is not exact (so `(/ 10 3)` is `3.3333...`, not `3`). For truncated integer division use [`math/quotient`](#math-quotient).

```sema
(/ 10 2)      ;; => 5
(/ 10 3)      ;; => 3.3333333333333335
(/ 10.0 3)    ;; => 3.3333333333333335
```
