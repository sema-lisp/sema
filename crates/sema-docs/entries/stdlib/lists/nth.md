---
name: "nth"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: any, doc: "list or vector" }, { name: n, type: int, doc: "zero-based index" }]
returns: "any"
---

Return the element at index N (zero-based).

```sema
(nth '(10 20 30) 1)   ; => 20
(nth '(10 20 30) 0)   ; => 10
```
