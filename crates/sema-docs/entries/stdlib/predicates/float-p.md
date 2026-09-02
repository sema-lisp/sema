---
name: "float?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
see_also: ["integer?", "number?", "inexact?", "type-of"]
---

Test if a value is a floating-point number.

```sema
(float? 3.14)   ; => #t
(float? 42)     ; => #f
```
