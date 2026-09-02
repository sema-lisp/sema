---
name: "make-list"
module: "lists"
section: "Construction"
params: [{ name: n, type: int }, { name: value, type: any }]
returns: "list"
see_also: ["list/repeat", "iota", "list"]
---

Alias for `list/repeat`.

```sema
(make-list 3 0)   ; => (0 0 0)
```
