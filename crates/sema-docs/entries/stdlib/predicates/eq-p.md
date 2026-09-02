---
name: "eq?"
module: "predicates"
section: "Equality"
params: [{ name: a, type: any }, { name: b, type: any }]
returns: "bool"
see_also: ["equal?", "=", "not"]
---

Test structural (deep) equality. `equal?` is an alias.

In Sema, unlike traditional Lisps, `eq?` compares values by structure rather than by object identity — so two distinct lists with the same contents are `eq?`. This makes it safe to compare freshly-built collections.

```sema
(eq? 'a 'a)           ; => #t
(eq? '(1 2) '(1 2))   ; => #t   (separate lists, same contents)
(eq? [1 [2 3]] [1 [2 3]])  ; => #t   (deep / nested)
(eq? 1 2)             ; => #f
```

See also `=`, which does *numeric* equality across int/float (`(= 1 1.0)` is `#t`) but falls back to structure for non-numbers; and `equal?`, an alias of `eq?`.
