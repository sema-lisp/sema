---
name: "list/intersect"
module: "lists"
section: "Set Operations"
params: [{ name: a, type: list }, { name: b, type: list }]
returns: "list"
see_also: ["list/diff", "list/unique", "list/contains?"]
---

Return elements present in both lists.

```sema
(list/intersect '(1 2 3 4 5) '(3 4 6))   ; => (3 4)
```
