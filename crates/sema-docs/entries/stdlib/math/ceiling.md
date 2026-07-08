---
name: "ceiling"
module: "math"
section: "Scheme Aliases"
---

Alias for `ceil` — the R7RS spelling. Same exactness-preserving behavior: a float rounds to a float, an exact rational rounds to an exact integer.

```sema
(ceiling 3.2)  ; => 4.0
(ceiling 7/2)  ; => 4     (exact rational -> exact integer)
```
