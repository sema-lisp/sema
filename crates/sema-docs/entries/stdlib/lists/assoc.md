---
name: "assoc"
module: "lists"
section: "Association Lists"
---

Look up a key in an association list (list of pairs). Uses `equal?` comparison.

```sema
(define alist '(("a" 1) ("b" 2) ("c" 3)))
(assoc "b" alist)   ; => ("b" 2)
(assoc "z" alist)   ; => #f
```
