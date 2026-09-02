---
name: "list/unique"
module: "lists"
section: "Searching"
params: [{ name: seq, type: list }]
returns: "list"
see_also: ["list/dedupe", "list/duplicates", "list/diff"]
---

Remove duplicate elements, keeping the **first** occurrence of each value so the original order is preserved (unlike `sort` + dedupe, which would reorder).

```sema
(list/unique '(1 2 2 3 3 3))   ; => (1 2 3)
(list/unique '(3 1 3 2 1))     ; => (3 1 2)
```

See also: `list/duplicates` (the opposite — only the values that repeat).
