---
name: "drop"
module: "lists"
section: "Sublists"
params: [{ name: n, type: int }, { name: lst, type: list }]
returns: "list"
see_also: ["take", "drop-while", "list/drop-last", "rest"]
---

Return the list with its first `n` elements removed. Dropping more than the list holds yields the empty list (no error). The counterpart to `take`.

```sema
(drop 2 '(1 2 3 4 5))   ; => (3 4 5)
(drop 10 '(1 2 3))      ; => ()
(drop 0 '(1 2 3))       ; => (1 2 3)
```

See also `drop-while` (drops a leading run by predicate) and `list/drop-last` (drops from the tail).

