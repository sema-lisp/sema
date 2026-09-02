---
name: "not"
module: "predicates"
section: "Logic"
params: [{ name: x, type: any }]
returns: bool
see_also: ["nil?", "bool?", "eq?"]
---

Logical negation. Returns `#t` when `x` is falsy (`#f` or `nil`) and `#f` otherwise.

```sema
(not #f)    ; => #t
(not nil)   ; => #t
(not 0)     ; => #f
(not '())   ; => #f
```
