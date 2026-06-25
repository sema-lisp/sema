---
name: "string?"
module: "predicates"
section: "Type Predicates"
params: [{ name: x, type: any }]
returns: "bool"
---

Test if a value is a string.

```sema
(string? "hi")   ; => #t
(string? 42)     ; => #f
```
