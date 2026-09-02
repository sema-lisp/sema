---
name: "assv"
module: "lists"
section: "Association Lists"
params: [{ name: obj, type: any }, { name: alist, type: list }]
returns: "list or #f"
see_also: ["assq", "assoc", "member", "list/find"]
---

Find the first pair in an association list whose key equals `key`. In Sema this
compares by value, so `assv`, `assq`, and `assoc` all match structurally equal
keys (including compound keys like lists) — they are not distinguished by
object identity the way Scheme's `eqv?`/`eq?` would be.

```sema
(assv 2 '((1 "one") (2 "two")))   ; => (2 "two")
```
