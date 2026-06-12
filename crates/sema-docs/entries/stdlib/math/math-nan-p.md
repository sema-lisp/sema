---
name: "math/nan?"
module: "math"
section: "Numeric Predicates"
---

Test if a value is NaN (not a number).

```sema
(math/nan? math/nan)       ; => #t
(math/nan? 42)             ; => #f
```
