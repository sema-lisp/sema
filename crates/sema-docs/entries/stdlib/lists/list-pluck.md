---
name: "list/pluck"
module: "lists"
section: "Extraction"
params: [{ name: key, type: any }, { name: seq, type: list }]
returns: "list"
see_also: ["map", "list/key-by", "get", "map/select-keys"]
---

Extract a specific key from each map in a list.

```sema
(define people (list {:name "Alice" :age 30} {:name "Bob" :age 25}))
(list/pluck :name people)   ; => ("Alice" "Bob")
```
