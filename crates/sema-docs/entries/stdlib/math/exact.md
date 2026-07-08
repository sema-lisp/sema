---
name: "exact"
module: "math"
section: "Exactness Conversion"
params: [{ name: x, type: number }]
returns: "number"
see_also: ["inexact", "inexact->exact", "exact->inexact"]
---

Convert a number to its exact form. A finite float is converted to the *exact* rational it actually represents (not a rounded-off approximation), then reduced and normalized to an integer when the denominator is 1. Inexact components of a complex number are converted the same way, and already-exact numbers are returned unchanged. [`inexact->exact`](../inexact-to-exact) is the longer R7RS spelling of this same operation.

```sema
(exact 0.5)       ; => 1/2
(exact 0.25)      ; => 1/4
(exact 2.0)       ; => 2                              ; normalizes to integer
(exact 1/3)       ; => 1/3                            ; already exact
(exact 3.0+4.0i)  ; => 3+4i
(exact 3.14159)   ; => 3537115888337719/1125899906842624
```

The last result looks surprising but is correct: `3.14159` is not exactly
representable in binary, so the stored double is a specific fraction with a
power-of-two denominator, and `exact` returns that fraction verbatim. To get a
tidy approximation like `22/7` instead, use [`rationalize`](../rationalize).
