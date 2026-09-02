---
name: "list/avg"
module: "lists"
section: "Statistics"
params: [{ name: seq, type: list }]
returns: "number"
see_also: ["list/sum", "list/median", "list/mode", "list/max"]
---

Return the average of a numeric list.

```sema
(list/avg '(2 4 6))   ; => 4.0
```
