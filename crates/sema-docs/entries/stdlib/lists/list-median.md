---
name: "list/median"
module: "lists"
section: "Statistics"
params: [{ name: seq, type: list }]
returns: "number"
see_also: ["list/avg", "list/mode", "list/sum"]
---

Return the statistical median.

```sema
(list/median '(3 1 2))     ; => 2.0
(list/median '(1 2 3 4))   ; => 2.5
```
