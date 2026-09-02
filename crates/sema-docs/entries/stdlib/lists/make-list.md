---
name: "make-list"
module: "lists"
section: "Construction"
params: [{ name: n, type: int }, { name: value, type: any }]
returns: "list"
see_also: ["list/repeat", "iota", "list"]
---

Build a list of `n` copies of `value`. This is the R7RS name for
`list/repeat`; both names run the same code. `n` must be a non-negative
integer, and `0` gives the empty list.

Every element is the *same* value, not a copy. For an immutable value that
does not matter; for a mutable container such as a `mutable-array`, all `n`
slots point at one container, so mutating through one slot is visible in all
of them. Use `list/times` with a constructor when each element must be fresh.

```sema
(make-list 3 0)          ; => (0 0 0)
(make-list 0 'x)         ; => ()
(make-list 2 (list 1))   ; => ((1) (1))
(list/times 2 (fn (i) (list i)))   ; => ((0) (1))
```

A count above the bulk-allocation limit (100 million elements) raises an
error instead of exhausting memory.
