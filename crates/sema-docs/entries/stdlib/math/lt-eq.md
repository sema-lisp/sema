---
name: "<="
module: "math"
section: "Comparison"
syntax: "(<= num ...)"
returns: "bool"
see_also: ["<", ">=", ">", "="]
---

Less than or equal. Variadic: `(<= a b c)` is true when arguments are in non-decreasing order.

```sema
(<= 1 2)      ; => #t
(<= 2 2)      ; => #t
(<= 1 2 2)    ; => #t   (non-decreasing chain)
```
