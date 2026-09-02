---
name: "list/repeat"
module: "lists"
section: "Construction"
params: [{ name: n, type: int }, { name: value, type: any }]
returns: "list"
see_also: ["make-list", "list/times", "iota", "list/pad"]
---

Create a list by repeating a value N times.

```sema
(list/repeat 3 0)   ; => (0 0 0)
(list/repeat 4 "x") ; => ("x" "x" "x" "x")
```
