---
name: "list/pluck"
module: "lists"
section: "Extraction"
---

Extract a specific key from each map in a list.

```sema
(define people (list {:name "Alice" :age 30} {:name "Bob" :age 25}))
(list/pluck :name people)   ; => ("Alice" "Bob")
```
