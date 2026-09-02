---
name: "list/find"
module: "lists"
section: "Filtering"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "any or nil"
see_also: ["filter", "list/sole", "list/index-of", "any"]
---

Return the first element that satisfies a predicate, or `nil` if none found.

```sema
(list/find even? '(1 3 4 5 6))   ; => 4
(list/find even? '(1 3 5))       ; => nil
```

See also: `list/sole` (asserts exactly one match, errors otherwise), `list/index-of` (position of a value), `list/find` returns the element while `member` returns the tail.
