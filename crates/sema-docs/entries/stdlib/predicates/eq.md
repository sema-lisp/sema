---
name: "="
module: "predicates"
section: "Equality"
syntax: "(= a b ...)"
returns: "bool"
---

Equality. For numbers this is numeric equality (so `(= 1 1.0)` is `#t`); for
non-numbers it falls back to structural equality. Unlike `<` / `>`, comparing
non-numbers does not error — it reports whether the values are equal.

```sema
(= 1 1)           ; => #t
(= 1 1.0)         ; => #t
(= 1 2)           ; => #f
(= "abc" "abc")   ; => #t   (structural, not an error)
(= 1 "a")         ; => #f
```
