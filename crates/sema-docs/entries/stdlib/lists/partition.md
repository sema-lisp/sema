---
name: "partition"
module: "lists"
section: "Sublists"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["filter", "list/reject", "list/group-by", "list/split-at"]
---

Split a list into two lists in a single pass: the elements that satisfy the predicate, then the elements that don't. Returns a list of exactly two lists, each preserving the original order.

This is the both-halves version of `filter` — instead of throwing away the non-matches like `filter` (or keeping only them like `list/reject`), you get both groups at once, which is handy with destructuring.

```sema
(partition even? '(1 2 3 4 5))   ; => ((2 4) (1 3 5))

(let (([evens odds] (partition even? '(1 2 3 4 5))))
  (str "evens=" evens " odds=" odds))   ; => "evens=(2 4) odds=(1 3 5)"
```

See also: `list/group-by` (split into N groups by an arbitrary key, not just two by a boolean).
