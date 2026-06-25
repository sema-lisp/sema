---
name: "="
module: "math"
section: "Comparison"
syntax: "(= a b ...)"
returns: "bool"
---

Equality. For numbers this is numeric equality (so `(= 1 1.0)` is `#t`); for
non-numbers it falls back to structural equality. Unlike `<` / `>`, comparing
non-numbers does not error.

```sema
(= 1 1)           ; => #t
(= 1 1.0)         ; => #t
(= 1 2)           ; => #f
(= "abc" "abc")   ; => #t   (structural, not an error)
```
