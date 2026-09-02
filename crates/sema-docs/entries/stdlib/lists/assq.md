---
name: "assq"
module: "lists"
section: "Association Lists"
params: [{ name: obj, type: any }, { name: alist, type: list }]
returns: "list or #f"
see_also: ["assv", "assoc", "member", "list/find"]
---

Like `assoc` but uses `eq?` comparison (pointer/symbol equality).

```sema
(assq 'b '((a 1) (b 2)))   ; => (b 2)
```
