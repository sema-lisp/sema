---
name: "every"
module: "lists"
section: "Searching"
---

Test if all elements satisfy a predicate.

```sema
(every even? '(2 4 6))     ; => #t
(every even? '(2 3 6))     ; => #f
```
