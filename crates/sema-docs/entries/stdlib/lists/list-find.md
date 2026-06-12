---
name: "list/find"
module: "lists"
section: "Filtering"
---

Return the first element that satisfies a predicate, or `nil` if none found.

```sema
(list/find even? '(1 3 4 5 6))   ; => 4
(list/find even? '(1 3 5))       ; => nil
```
