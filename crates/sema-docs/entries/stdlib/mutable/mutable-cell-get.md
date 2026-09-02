---
name: "mutable-cell/get"
module: "mutable"
section: "Mutable Containers"
params: [{ name: cell, type: mutable-cell }]
returns: "any"
see_also: ["mutable-cell/set!", "mutable-cell/new"]
---

Read the current contents of a mutable cell.

```sema
(define c (mutable-cell/new :ready))
(mutable-cell/get c)   ; => :ready
```
