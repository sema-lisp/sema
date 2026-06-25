---
name: "apply"
module: "lists"
section: "Higher-Order Functions"
syntax: "(apply f arg ... lst)"
returns: "any"
---

Apply a function to a list of arguments.

```sema
(apply + '(1 2 3))   ; => 6
(apply max '(3 1 4)) ; => 4
```
