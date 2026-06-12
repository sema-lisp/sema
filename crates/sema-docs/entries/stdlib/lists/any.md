---
name: "any"
module: "lists"
section: "Searching"
---

Test if any element satisfies a predicate.

```sema
(any even? '(1 3 5 6))   ; => #t
(any even? '(1 3 5))     ; => #f
```
