---
name: "mutable-cell/new"
module: "mutable"
section: "Mutable Containers"
params: [{ name: value, type: any, doc: "initial contents" }]
returns: "mutable-cell"
see_also: ["mutable-cell/get", "mutable-cell/set!", "mutable-array/new"]
---

Create a mutable cell: a single in-place mutable slot holding one value (a boxed value). Like mutable arrays, cells are shared by reference — the imperative escape hatch for a counter or running aggregate threaded through callbacks without rebuilding a container per update. Cells cannot be used as map keys.

```sema
(define counter (mutable-cell/new 0))
(mutable-cell/set! counter (+ 1 (mutable-cell/get counter)))
(mutable-cell/get counter)   ; => 1
```
