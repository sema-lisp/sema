---
name: "assv"
module: "lists"
section: "Association Lists"
---

Like `assoc` but uses `eqv?` comparison (value equality for numbers).

```sema
(assv 2 '((1 "one") (2 "two")))   ; => (2 "two")
```
