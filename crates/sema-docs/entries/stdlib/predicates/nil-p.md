---
name: "nil?"
module: "predicates"
section: "Emptiness Predicates"
params: [{ name: value, type: any }]
returns: "bool"
see_also: ["null?", "empty?", "not"]
---

Test if a value is `nil` specifically (not the empty list).

```sema
(nil? nil)     ;; => #t
(nil? '())     ;; => #f
(nil? 0)       ;; => #f
```

Stricter than `null?`, which is also true for the empty list. Use `nil?` when you specifically need to distinguish "no value" from an empty collection.
