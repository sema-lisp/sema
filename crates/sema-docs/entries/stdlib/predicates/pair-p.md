---
name: "pair?"
module: "predicates"
section: "Collection Predicates"
params: [{ name: x, type: any }]
returns: "bool"
see_also: ["list?", "null?", "cons"]
---

Test if a value is a non-empty list (Scheme compatibility).

```sema
(pair? '(1 2))   ; => #t
(pair? '())      ; => #f   (empty list has no head/tail)
(pair? '(1))     ; => #t
```

Differs from `list?` only on the empty list: `(list? '())` is `#t` but `(pair? '())` is `#f`. Guard recursive list-walking with `pair?` so you stop at the empty tail.
