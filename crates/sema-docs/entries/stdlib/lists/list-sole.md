---
name: "list/sole"
module: "lists"
section: "Filtering"
---

Return the single element matching a predicate. Errors if zero or more than one match.

```sema
(list/sole (fn (x) (> x 4)) '(1 2 3 4 5))   ; => 5
```
