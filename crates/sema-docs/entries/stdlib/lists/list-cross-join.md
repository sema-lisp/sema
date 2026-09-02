---
name: "list/cross-join"
module: "lists"
section: "Windowing"
params: [{ name: a, type: list }, { name: b, type: list }]
returns: "list"
see_also: ["zip", "list/interleave", "list/intersect"]
---

Cartesian product of two lists.

```sema
(list/cross-join '(1 2) '(3 4))   ; => ((1 3) (1 4) (2 3) (2 4))
```
