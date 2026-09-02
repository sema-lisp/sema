---
name: "list/diff"
module: "lists"
section: "Set Operations"
params: [{ name: a, type: list }, { name: b, type: list }]
returns: "list"
see_also: ["list/intersect", "list/unique", "list/contains?"]
---

Return elements in the first list that are not in the second list.

```sema
(list/diff '(1 2 3 4 5) '(3 4))   ; => (1 2 5)
```
