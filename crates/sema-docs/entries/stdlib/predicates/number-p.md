---
name: "number?"
module: "predicates"
section: "Numeric Predicates"
params: [{ name: v, type: any }]
returns: "bool"
---

Test if a value is a number (integer or float).

```sema
(number? 42)     ; => #t
(number? 3.14)   ; => #t
(number? "42")   ; => #f
```
