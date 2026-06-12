---
name: "math/pow"
module: "math"
params: [{ name: base, type: number }, { name: exponent, type: number }]
returns: "number"
---

Raise `base` to `exponent`. With two non-negative integers the result is an integer; otherwise both operands are treated as floats and a float is returned. Namespaced form of `pow`/`expt`.

```sema
(math/pow 2 10)     ; => 1024
(math/pow 2.0 0.5)  ; => 1.4142135623730951
```
