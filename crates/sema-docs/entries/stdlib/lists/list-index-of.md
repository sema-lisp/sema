---
name: "list/index-of"
module: "lists"
section: "Searching"
params: [{ name: seq, type: list }, { name: value, type: any }]
returns: "int or nil"
see_also: ["list/contains?", "member", "list/find", "nth"]
---

Return the index of the first occurrence of a value, or `nil` if not found.

```sema
(list/index-of '(10 20 30) 20)   ;; => 1
(list/index-of '(10 20 30) 99)   ;; => nil
```
