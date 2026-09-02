---
name: "take"
module: "lists"
section: "Sublists"
params: [{ name: n, type: int }, { name: lst, type: list }]
returns: "list"
see_also: ["drop", "take-while", "list/take-last", "first"]
---

Take the first N elements (the count comes first: `(take n seq)`). Asking for more than exist is clamped to the whole list rather than erroring.

```sema
(take 3 '(1 2 3 4 5))   ; => (1 2 3)
(take 10 '(1 2))        ; => (1 2)   ; clamped, no error
```

See also: `list/take-last` (from the end), `take-while`/`list/take-while` (stop on a predicate), `list/split-at` (both halves at an index).
