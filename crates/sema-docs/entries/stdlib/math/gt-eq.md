---
name: ">="
module: "math"
section: "Comparison"
syntax: "(>= num ...)"
returns: "bool"
see_also: [">", "<=", "<", "="]
---

Greater than or equal. Variadic: `(>= a b c)` is true when arguments are in non-increasing order.

```sema
(>= 3 2)      ; => #t
(>= 2 2)      ; => #t
(>= 3 2 2)    ; => #t   (non-increasing chain)
```
