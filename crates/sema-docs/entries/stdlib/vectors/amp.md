---
name: "&"
module: "vectors"
section: "Destructuring"
---

```sema
(let (([head second & tail] [1 2 3 4 5]))
  [head second tail])
; => [1 2 (3 4 5)]
```

Note: `tail` is a **list**, not a vector.
