---
name: "length"
module: "vectors"
section: "Predicates & Introspection"
aliases: ["count", "empty?"]
---

Vectors participate in Sema's generic collection functions:

```sema
(length [10 20 30])   ; => 3
(count [10 20 30])    ; => 3
(empty? [])           ; => #t
(empty? [1])          ; => #f
```
