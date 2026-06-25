---
name: "math/random-int"
module: "math"
section: "Random Numbers"
params: [{ name: lo, type: int, doc: "inclusive lower bound" }, { name: hi, type: int, doc: "inclusive upper bound" }]
returns: "int"
---

Return a random integer in a range (inclusive on both ends).

```sema
(math/random-int 1 100)  ; => 42 (varies)
(math/random-int 0 9)    ; => 7 (varies)
```
