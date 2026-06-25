---
name: "any"
module: "lists"
section: "Searching"
params: [{ name: pred, type: function }, { name: lst, type: list }]
returns: "bool"
---

Test if any element satisfies a predicate.

```sema
(any even? '(1 3 5 6))   ; => #t
(any even? '(1 3 5))     ; => #f
```
