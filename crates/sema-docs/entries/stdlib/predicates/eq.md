---
name: "="
module: "predicates"
section: "Equality"
syntax: "(= a b ...)"
returns: "bool"
---

Equality. For numbers this is **numeric** equality, so `(= 1 1.0)` is `#t`
even though the int and float differ in representation. For non-numbers it falls
back to **structural** equality (recursing into lists, maps, vectors).

`=` never errors on type mismatches — it just answers `#f` (or `#t`). That makes
it the safe general-purpose equality. Contrast with `eq?`, which is stricter
identity/value equality: `(eq? 1 1.0)` is `#f` because the values are not the
same kind. Reach for `=` when comparing numbers or whole data structures; reach
for `eq?` when you want "is this the exact same value" (no int/float coercion).

```sema
(= 1 1)             ; => #t
(= 1 1.0)           ; => #t   (numeric: int = float)
(= 1 2)             ; => #f
(= "abc" "abc")     ; => #t   (structural)
(= '(1 2) '(1 2))   ; => #t   (recurses into the list)

(eq? 1 1.0)         ; => #f   (no numeric coercion)
```

See also: `eq?` (value identity), `<` / `>` (ordering — these *do* error on mixed
or unorderable types, where `=` does not).
