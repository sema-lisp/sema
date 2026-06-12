---
name: "assq"
module: "lists"
section: "Association Lists"
---

Like `assoc` but uses `eq?` comparison (pointer/symbol equality).

```sema
(assq 'b '((a 1) (b 2)))   ; => (b 2)
```
