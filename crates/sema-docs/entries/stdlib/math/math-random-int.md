---
name: "math/random-int"
module: "math"
section: "Random Numbers"
params: [{ name: lo, type: int, doc: "inclusive lower bound" }, { name: hi, type: int, doc: "inclusive upper bound" }]
returns: "int"
see_also: ["math/random"]
---

Return a random integer in `[lo, hi]`, inclusive on *both* ends — `(math/random-int 1 6)` can return 6. `lo` must be `<= hi` or it errors. The generator is not seedable, so results are not reproducible across runs.

```sema
(math/random-int 1 100)  ; => 42 (varies)
(math/random-int 0 9)    ; => 7 (varies)
(math/random-int 6 1)    ; error: lo (6) must be <= hi (1)
```

For a random element of a collection, combine with the length: `(nth coll (math/random-int 0 (- (length coll) 1)))`.
