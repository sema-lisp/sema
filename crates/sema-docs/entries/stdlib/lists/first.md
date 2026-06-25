---
name: "first"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: list, doc: "list or vector; nil if empty" }]
returns: "any"
---

Alias for `car`. Return the first element.

```sema
(first '(1 2 3))   ; => 1
```
