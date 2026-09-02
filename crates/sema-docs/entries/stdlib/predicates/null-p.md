---
name: "null?"
module: "predicates"
section: "Emptiness Predicates"
params: [{ name: x, type: any }]
returns: "bool"
see_also: ["nil?", "empty?", "list?", "pair?"]
---

Test if a value is the empty list or `nil`.

```sema
(null? '())    ;; => #t
(null? nil)    ;; => #t
(null? '(1))   ;; => #f
(null? "")     ;; => #f   (empty string is not null)
```

Three closely related tests: `null?` is true for `nil` *and* the empty list (the Scheme-style end-of-list check); `nil?` is true only for `nil`; `empty?` is broader, true for any empty collection or string (and `nil`).
