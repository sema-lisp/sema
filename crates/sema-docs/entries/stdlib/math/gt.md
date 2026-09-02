---
name: ">"
module: "math"
section: "Comparison"
syntax: "(> num ...)"
returns: "bool"
see_also: [">=", "<", "<=", "="]
---

Greater than. Variadic: `(> a b c)` is true when arguments are in strictly descending order.

```sema
(> 3 2)       ; => #t
(> 1 2)       ; => #f
(> 3 2 1)     ; => #t   (each > the next)
```
