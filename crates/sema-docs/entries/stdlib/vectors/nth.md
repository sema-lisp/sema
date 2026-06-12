---
name: "nth"
module: "vectors"
section: "Indexed Access"
---

Return the element at index `n` (zero-based). Works on both lists and vectors.

```sema
(nth [10 20 30] 0)    ; => 10
(nth [10 20 30] 2)    ; => 30
```

Out of bounds is an error:

```sema
(nth [10 20 30] 3)
; => error: index 3 out of bounds (length 3)
```

Use `first` for safe "index 0" access — it returns `nil` on empty sequences.
