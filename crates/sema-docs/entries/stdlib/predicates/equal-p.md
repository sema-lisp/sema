---
name: "equal?"
module: "predicates"
section: "Equality"
params: [{ name: a, type: any }, { name: b, type: any }]
returns: bool
see_also: ["eq?", "=", "not"]
---

Test structural (deep) equality of two values.

```sema
(equal? '(1 2) '(1 2))   ; => #t
(equal? "ab" "ab")       ; => #t
(equal? 1 2)             ; => #f
```

Alias of `eq?`. Note it is *not* numeric: `(equal? 1 1.0)` is `#f` because an int and a float differ structurally — use `=` when you want `1` and `1.0` to compare equal.
