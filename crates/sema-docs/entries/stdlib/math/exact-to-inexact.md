---
name: "exact->inexact"
module: "math"
section: "Exactness Conversion"
params: [{ name: x, type: number }]
returns: "number"
see_also: ["inexact", "exact", "inexact->exact"]
---

Convert a number to inexact (floating-point) form. Every component is converted to `f64`, so an exact rational becomes its nearest double and a complex number's parts each become floats. This is the R7RS spelling; [`inexact`](../inexact) is the shorthand for the same operation and either name works.

```sema
(exact->inexact 1/3)   ; => 0.3333333333333333   ; nearest f64
(exact->inexact 42)    ; => 42.0
(exact->inexact 3+4i)  ; => 3.0+4.0i
```

The inverse direction is [`inexact->exact`](../inexact-to-exact) / [`exact`](../exact).
