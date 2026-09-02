---
name: "list"
module: "lists"
section: "Construction & Access"
syntax: "(list item ...)"
returns: "list"
see_also: ["cons", "append", "make-list", "vector"]
---

Create a new list.

```sema
(list 1 2 3)       ; => (1 2 3)
(list)             ; => ()
(list "a" "b")     ; => ("a" "b")
```
