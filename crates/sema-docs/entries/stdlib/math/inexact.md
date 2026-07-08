---
name: "inexact"
module: "math"
section: "Exactness Conversion"
params: [{ name: x, type: number }]
returns: "number"
see_also: ["exact", "exact->inexact", "inexact->exact"]
---

Convert a number to inexact (floating-point) form. Every component is converted to `f64`, so an exact rational becomes its nearest double and each part of a complex number becomes a float. Useful for forcing floating-point arithmetic or triggering inexact contagion. [`exact->inexact`](../exact-to-inexact) is the longer R7RS spelling of the same operation.

```sema
(inexact 1/3)   ; => 0.3333333333333333   ; nearest f64
(inexact 42)    ; => 42.0
(inexact 3+4i)  ; => 3.0+4.0i
(inexact 3.14)  ; => 3.14                  ; already inexact
```

The inverse direction is [`exact`](../exact) / [`inexact->exact`](../inexact-to-exact).
