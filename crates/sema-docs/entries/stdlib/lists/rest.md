---
name: "rest"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: list }]
returns: "list"
---

Alias for `cdr`. Return the rest of the list.

```sema
(rest '(1 2 3))    ; => (2 3)
```
