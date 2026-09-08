---
name: "list/avg"
module: "lists"
section: "Statistics"
params: [{ name: seq, type: list }]
returns: "number"
see_also: ["list/sum", "list/median", "list/mode", "list/max"]
---

Return the average of a numeric list. Exact inputs produce an exact result;
an inexact input produces an inexact result.

```sema
(list/avg '(2 4 6))       ; => 4
(list/avg '(1 2 3 4))     ; => 5/2
(list/avg '(1 2.0 3))     ; => 2.0
```
