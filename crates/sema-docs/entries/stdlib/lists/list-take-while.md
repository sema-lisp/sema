---
name: "list/take-while"
module: "lists"
section: "Splitting"
---

Take elements from the front while a predicate holds.

```sema
(list/take-while (fn (x) (< x 4)) '(1 2 3 4 5))   ; => (1 2 3)
```
