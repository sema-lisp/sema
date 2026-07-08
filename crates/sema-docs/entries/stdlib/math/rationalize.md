---
name: "rationalize"
module: "math"
section: "Rational Approximation"
params: [{ name: x, type: number }, { name: tol, type: number }]
returns: "number"
see_also: ["exact", "inexact", "denominator", "numerator"]
---

Find the *simplest* rational number within `tol` of `x` — the one with the smallest denominator anywhere in the interval `[x - |tol|, x + |tol|]`. This is the R7RS `rationalize`, handy for turning a messy value into a readable fraction like `22/7`.

Exactness follows R7RS contagion: the result is exact only when **both** `x` and `tol` are exact. If either argument is inexact, the answer is an inexact real (the simplest rational, rendered as a float).

```sema
(rationalize 1/3 1/1000)            ; => 1/3     ; exact in → exact out
(rationalize (exact 3.14159) 1/100) ; => 22/7    ; simplest fraction near π
(rationalize 3/10 1/10)             ; => 1/3     ; 1/3 is simpler than 3/10
(rationalize 3.14159 1/100)         ; => 3.142857142857143   ; inexact in → inexact out (22/7 as f64)
```

The last example shows contagion: because `3.14159` is inexact, `rationalize`
still finds `22/7` but returns it as a float. An inexact *tolerance* has the same
effect — `(rationalize 1/2 0.01)` yields `0.5`, not `1/2`. Use [`exact`](../exact)
on the inputs first when you want an exact fraction back.
