---
name: "for-each"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function }, { name: list, type: list }]
returns: "nil"
---

Apply a function to each element for side effects.

```sema
(for-each println '("a" "b" "c"))
;; prints: a, b, c (each on a new line)
```
