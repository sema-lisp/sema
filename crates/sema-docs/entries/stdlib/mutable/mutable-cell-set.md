---
name: "mutable-cell/set!"
module: "mutable"
section: "Mutable Containers"
params: [{ name: cell, type: mutable-cell }, { name: value, type: any }]
returns: "mutable-cell"
see_also: ["mutable-cell/get", "mutable-cell/new"]
---

Replace the contents of a mutable cell, in place. Returns the cell. Every binding to the cell sees the new value.

```sema
(define best (mutable-cell/new nil))
(mutable-cell/set! best 42)
(mutable-cell/get best)   ; => 42
```
