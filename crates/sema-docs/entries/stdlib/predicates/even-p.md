---
name: "even?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: n, type: int }]
returns: "bool"
---

Test if an integer is even. Requires an integer — passing a float raises a type error. See `odd?` for the complement.

```sema
(even? 4)    ; => #t
(even? 3)    ; => #f
(even? -2)   ; => #t   (sign doesn't matter)
(even? 0)    ; => #t
```
