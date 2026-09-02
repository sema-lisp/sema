---
name: "list/min"
module: "lists"
section: "Aggregation"
params: [{ name: seq, type: list }]
returns: "number"
see_also: ["list/max", "min", "list/sum", "list/avg"]
---

Return the smallest value in a list. Errors on an empty list (there is no minimum to return), so guard or default empty input yourself.

```sema
(list/min '(3 1 4 1 5))   ; => 1
(list/min '(2.5 1.5 3))   ; => 1.5
```
