---
name: "list/sum"
module: "lists"
section: "Aggregation"
params: [{ name: seq, type: list }]
returns: "number"
see_also: ["list/avg", "list/max", "list/min", "foldl"]
---

Sum all numbers in a list.

```sema
(list/sum '(1 2 3 4 5))   ; => 15
```
