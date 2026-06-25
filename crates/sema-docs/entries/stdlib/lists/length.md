---
name: "length"
module: "lists"
section: "Basic Operations"
params: [{ name: list, type: list }]
returns: "int"
---

Return the number of elements in a list.

```sema
(length '(1 2 3))  ; => 3
(length '())       ; => 0
```
