---
name: "member"
module: "lists"
section: "Searching"
---

Return the tail of the list starting from the first matching element.

```sema
(member 3 '(1 2 3 4))   ; => (3 4)
(member 9 '(1 2 3))     ; => #f
```
