---
name: "odd?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: n, type: int }]
returns: "bool"
see_also: ["even?", "zero?", "integer?"]
---

Test if an integer is odd. Requires an integer — passing a float raises a type error.

```sema
(odd? 3)    ; => #t
(odd? 4)    ; => #f
(odd? -3)   ; => #t   (sign doesn't matter)
(odd? 0)    ; => #f
```
