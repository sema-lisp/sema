---
name: "list/mode"
module: "lists"
section: "Statistics"
params: [{ name: seq, type: list }]
returns: "any or list"
see_also: ["frequencies", "list/median", "list/avg"]
---

Return the most frequent value. If tied, returns a list.

```sema
(list/mode '(1 2 2 3 3 3))   ; => 3
(list/mode '(1 1 2 2))       ; => (1 2)
```
