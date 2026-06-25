---
name: "math/remainder"
module: "math"
section: "Integer Math"
params: [{ name: a, type: int }, { name: b, type: int }]
returns: "int"
---

Remainder after truncated division.

```sema
(math/remainder 10 3) ; => 1
(math/remainder 7 2)  ; => 1
```
