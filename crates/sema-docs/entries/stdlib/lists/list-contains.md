---
name: "list/contains?"
module: "lists"
section: "Access & Search"
params: [{ name: seq, type: list }, { name: value, type: any }]
returns: "bool"
see_also: ["member", "list/index-of", "list/find", "contains?"]
---

Return `#t` if the sequence contains `elem` (structural equality), else `#f`. Unlike `member` (which returns the Scheme-style tail or `#f`), this reads as a predicate and allocates nothing.

```sema
(list/contains? (list 1 2 3) 2)   ; => #t
(list/contains? (list 1 2 3) 9)   ; => #f
```
