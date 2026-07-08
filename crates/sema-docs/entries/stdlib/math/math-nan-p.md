---
name: "math/nan?"
module: "math"
section: "Numeric Predicates"
---

Test whether a value is NaN ("not a number"). This is the *only* reliable NaN check: NaN is never equal to anything, including itself, so `(= x math/nan)` is always `#f`. NaN arises from undefined float operations like `(/ 0.0 0.0)`.

```sema
(math/nan? math/nan)        ; => #t
(math/nan? 42)              ; => #f
(math/nan? (/ 0.0 0.0))     ; => #t
(math/nan? (sqrt -1))       ; => #f   ; negative sqrt is now complex (0+1i), not NaN
(= math/nan math/nan)       ; => #f   ; why you need this predicate
```
