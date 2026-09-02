---
name: "hash-map?"
module: "maps"
section: "Predicates"
params: [{ name: x, type: any }]
returns: bool
see_also: ["map?", "hashmap/new", "type-of"]
---

Return `#t` if `x` is an ordered map (the `{...}` / `map/new` type). Alias of `map?`. Note it returns `#f` for a `hashmap/new` value — that distinct `:hashmap` type has no dedicated predicate; test it with `(eq? (type x) :hashmap)`.

```sema
(hash-map? {:a 1})              ; => #t
(hash-map? '(1 2))             ; => #f
(hash-map? (hashmap/new :a 1)) ; => #f  (hashmap is a different type)
```
