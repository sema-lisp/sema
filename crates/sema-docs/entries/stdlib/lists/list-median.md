---
name: "list/median"
module: "lists"
section: "Statistics"
params: [{ name: seq, type: list }]
returns: "number"
see_also: ["list/avg", "list/mode", "list/sum"]
---

Return the statistical median of real numbers. Exact inputs produce an exact
result; an inexact input produces an inexact result.

```sema
(list/median '(3 1 2))     ; => 2
(list/median '(1 2 3 4))   ; => 5/2
```
