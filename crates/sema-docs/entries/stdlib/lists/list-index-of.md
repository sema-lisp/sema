---
name: "list/index-of"
module: "lists"
section: "Searching"
---

Return the index of the first occurrence of a value, or `nil` if not found.

```sema
(list/index-of '(10 20 30) 20)   ;; => 1
(list/index-of '(10 20 30) 99)   ;; => nil
```
