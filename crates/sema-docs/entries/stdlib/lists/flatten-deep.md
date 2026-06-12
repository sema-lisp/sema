---
name: "flatten-deep"
module: "lists"
section: "Sublists"
---

Recursively flatten all nested lists.

```sema
(flatten-deep '(1 (2 (3 (4)))))   ; => (1 2 3 4)
```
