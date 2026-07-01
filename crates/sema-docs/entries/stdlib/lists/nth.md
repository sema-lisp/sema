---
name: "nth"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: any, doc: "list or vector" }, { name: n, type: int, doc: "zero-based index" }]
returns: "any"
---

Return the element at index N (zero-based). Works on both lists and vectors.

```sema
(nth '(10 20 30) 1)   ; => 20
(nth '(10 20 30) 0)   ; => 10
(nth [10 20 30] 2)    ; => 30
```

Out of bounds is an error:

```sema
(nth [10 20 30] 3)
; => error: index 3 out of bounds (length 3)
```

Use `first` for safe "index 0" access — it returns `nil` on empty sequences.
