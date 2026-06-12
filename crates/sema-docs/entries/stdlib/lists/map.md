---
name: "map"
module: "lists"
section: "Higher-Order Functions"
---

Apply a function to each element of one or more lists.

```sema
(map (fn (x) (* x x)) '(1 2 3))      ; => (1 4 9)
(map + '(1 2 3) '(10 20 30))          ; => (11 22 33)
```
