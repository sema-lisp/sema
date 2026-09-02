---
name: "append"
module: "lists"
section: "Basic Operations"
syntax: "(append list ...)"
returns: "list"
see_also: ["cons", "list", "flatten", "zip"]
---

Concatenate lists.

```sema
(append '(1 2) '(3 4))     ; => (1 2 3 4)
(append '(1) '(2) '(3))    ; => (1 2 3)
```
