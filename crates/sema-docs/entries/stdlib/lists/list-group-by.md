---
name: "list/group-by"
module: "lists"
section: "Grouping"
---

Group elements by a function, returning a map.

```sema
(list/group-by even? '(1 2 3 4 5))   ; => {#f (1 3 5) #t (2 4)}
```
