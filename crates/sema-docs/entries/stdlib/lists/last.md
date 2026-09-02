---
name: "last"
module: "lists"
section: "Construction & Access"
params: [{ name: list, type: list }]
returns: "any"
see_also: ["first", "list/take-last", "nth", "car"]
---

Return the last element of a list.

```sema
(last '(1 2 3))    ; => 3
```
