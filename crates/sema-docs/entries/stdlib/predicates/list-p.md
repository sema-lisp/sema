---
name: "list?"
module: "predicates"
section: "Collection Predicates"
params: [{ name: x, type: any }]
returns: "bool"
see_also: ["pair?", "null?", "vector?", "empty?"]
---

Test if a value is a list (including the empty list).

```sema
(list? '(1))    ; => #t
(list? '())     ; => #t
(list? [1 2])   ; => #f   (a vector, not a list)
(list? 42)      ; => #f
```

`list?` is true for the empty list, whereas `pair?` requires a non-empty list. Vectors (`[...]`) are a separate type — test those with `vector?`.
