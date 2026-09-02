---
name: "flatten-deep"
module: "lists"
section: "Sublists"
params: [{ name: seq, type: list }]
returns: "list"
see_also: ["flatten", "flat-map", "append"]
---

Recursively flatten all nested lists.

```sema
(flatten-deep '(1 (2 (3 (4)))))   ; => (1 2 3 4)
```
