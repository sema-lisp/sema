---
name: "cons"
module: "lists"
section: "Construction & Access"
params: [{ name: x, type: any }, { name: lst, type: list }]
returns: "list"
---

Prepend an element to a list.

```sema
(cons 0 '(1 2 3))  ; => (0 1 2 3)
(cons 1 '())       ; => (1)
```
