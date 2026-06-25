---
name: "frequencies"
module: "lists"
section: "Grouping"
params: [{ name: list, type: list }]
returns: "map"
---

Count occurrences of each element, returning a map.

```sema
(frequencies '(a b a c b a))   ; => {a 3 b 2 c 1}
```
