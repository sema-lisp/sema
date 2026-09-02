---
name: "list/pick"
module: "lists"
section: "Random"
params: [{ name: seq, type: list }]
returns: "any"
see_also: ["list/shuffle", "first", "nth"]
---

Pick a random element from a list.

```sema
(list/pick '(1 2 3 4 5))   ; => 3 (varies)
```
