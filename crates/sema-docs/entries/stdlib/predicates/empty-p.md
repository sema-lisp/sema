---
name: "empty?"
module: "predicates"
section: "Emptiness Predicates"
params: [{ name: coll, type: any }]
returns: "bool"
see_also: ["null?", "length", "count", "nil?"]
---

Test if a collection, string, or `nil` is empty. Accepts strings, lists, vectors, maps, and `nil`.

```sema
(empty? "")        ;; => #t
(empty? '())       ;; => #t
(empty? nil)       ;; => #t
(empty? "hello")   ;; => #f
(empty? [1 2 3])   ;; => #f
```
