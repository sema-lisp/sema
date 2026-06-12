---
name: "list/key-by"
module: "lists"
section: "Extraction"
---

Transform a list of maps into a map keyed by a function result.

```sema
(list/key-by (fn (p) (get p :id)) people)   ; => map keyed by :id
```
