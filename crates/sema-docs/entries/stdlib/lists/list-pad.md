---
name: "list/pad"
module: "lists"
section: "Padding & Joining"
params: [{ name: seq, type: list }, { name: n, type: int }, { name: fill, type: any }]
returns: "list"
see_also: ["list/repeat", "length", "list/join"]
---

Pad a list to a target length with a fill value.

```sema
(list/pad '(1 2 3) 5 0)   ; => (1 2 3 0 0)
```
