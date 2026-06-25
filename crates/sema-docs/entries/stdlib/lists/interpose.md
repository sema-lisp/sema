---
name: "interpose"
module: "lists"
section: "Grouping"
params: [{ name: sep, type: any }, { name: list, type: list }]
returns: "list"
---

Insert a separator between elements.

```sema
(interpose ", " '("a" "b" "c"))   ; => ("a" ", " "b" ", " "c")
```
