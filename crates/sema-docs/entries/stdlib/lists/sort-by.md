---
name: "sort-by"
module: "lists"
section: "Higher-Order Functions"
---

Sort a list by a key function.

```sema
(sort-by length '("bb" "a" "ccc"))   ; => ("a" "bb" "ccc")
(sort-by abs '(-3 1 -2))             ; => (1 -2 -3)
```
