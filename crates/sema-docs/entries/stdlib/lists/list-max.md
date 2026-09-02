---
name: "list/max"
module: "lists"
section: "Aggregation"
params: [{ name: seq, type: list }]
returns: "number"
see_also: ["list/min", "max", "list/sum", "list/avg"]
---

Return the largest value in a list. Errors on an empty list (there is no maximum to return), so guard or default empty input yourself.

```sema
(list/max '(3 1 4 1 5))   ; => 5
(list/max '(2.5 1.5 3))   ; => 3
```
