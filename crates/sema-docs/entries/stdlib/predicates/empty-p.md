---
name: "empty?"
module: "predicates"
section: "Emptiness Predicates"
---

Test if a collection, string, or `nil` is empty. Accepts strings, lists, vectors, maps, and `nil`.

```sema
(empty? "")        ;; => #t
(empty? '())       ;; => #t
(empty? nil)       ;; => #t
(empty? "hello")   ;; => #f
(empty? [1 2 3])   ;; => #f
```
