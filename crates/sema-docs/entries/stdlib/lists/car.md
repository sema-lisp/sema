---
name: "car"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: list }]
returns: "any"
see_also: ["cdr", "first", "cadr", "nth"]
---

Return the first element of a list.

```sema
(car '(1 2 3))     ; => 1
```
