---
name: "every"
module: "lists"
section: "Searching"
params: [{ name: pred, type: function }, { name: lst, type: list }]
returns: "bool"
---

Test if all elements satisfy a predicate.

```sema
(every even? '(2 4 6))     ; => #t
(every even? '(2 3 6))     ; => #f
```
