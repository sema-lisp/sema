---
name: "list/dedupe"
module: "lists"
section: "Searching"
params: [{ name: seq, type: list }]
returns: "list"
see_also: ["list/unique", "list/duplicates", "frequencies"]
---

Collapse runs of *consecutive* equal elements into one. This is not a full set-uniq: a value that reappears after a different element survives (note the trailing `2` below). Sort first if you want every duplicate gone.

```sema
(list/dedupe '(1 1 2 2 3 3 2))         ; => (1 2 3 2)
(list/dedupe (sort '(1 1 2 2 3 3 2)))  ; => (1 2 3)
```

