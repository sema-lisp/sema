---
name: "flatten"
module: "lists"
section: "Sublists"
---

Flatten one level of nesting: splice each immediate sublist/vector element into the result.
(It is shallow — deeper nesting is preserved.)

```sema
(flatten '(1 (2 3) 4))     ; => (1 2 3 4)
(flatten '(1 (2 (3)) 4))   ; => (1 2 (3) 4)
```
