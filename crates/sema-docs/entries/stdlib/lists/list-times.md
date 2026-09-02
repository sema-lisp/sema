---
name: "list/times"
module: "lists"
section: "Generation"
params: [{ name: n, type: int }, { name: f, type: function }]
returns: "list"
see_also: ["list/repeat", "iota", "range", "map-indexed"]
---

Generate a list by calling a function N times with the index (0-based).

```sema
(list/times 5 (fn (i) (* i i)))   ; => (0 1 4 9 16)
```
