---
name: "math/random"
module: "math"
section: "Random Numbers"
syntax: "(math/random)"
returns: "float"
see_also: ["math/random-int"]
---

Return a random float in `[0.0, 1.0)` — 0.0 is possible, 1.0 never is. Not seedable, so draws are not reproducible. For a random integer, use [`math/random-int`](#math-random-int).

```sema
(math/random)              ; => 0.7291... (varies)
(* (math/random) 10)       ; => a float in [0.0, 10.0)
(< (math/random) 0.5)      ; => #t or #f — a fair coin flip
```
