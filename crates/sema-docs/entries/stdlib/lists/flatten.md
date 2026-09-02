---
name: "flatten"
module: "lists"
section: "Sublists"
params: [{ name: seq, type: list }]
returns: "list"
see_also: ["flatten-deep", "flat-map", "append"]
---

Flatten one level of nesting: splice each immediate sublist/vector element into the result.
(It is shallow — deeper nesting is preserved.)

```sema
(flatten '(1 (2 3) 4))     ; => (1 2 3 4)
(flatten '(1 (2 (3)) 4))   ; => (1 2 (3) 4)
```
