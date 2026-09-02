---
name: "math/round-to"
module: "math"
section: "Rounding"
params: [{ name: n, type: number }, { name: places, type: int, doc: "number of decimal digits" }]
returns: "float"
see_also: ["round", "math/format-fixed"]
---

Round a number to `places` decimal places, returning a float. (The core `round` only rounds to a whole integer.)

```sema
(math/round-to 3.14159 2)   ; => 3.14
(math/round-to 0.46666 3)   ; => 0.467
```
