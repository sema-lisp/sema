---
name: "flat-map"
module: "lists"
section: "Higher-Order Functions"
---

Map a function over a list and flatten the results by one level.

```sema
(flat-map (fn (x) (list x (* x 10))) '(1 2 3))
; => (1 10 2 20 3 30)
```
