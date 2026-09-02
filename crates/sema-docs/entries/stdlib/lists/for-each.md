---
name: "for-each"
module: "lists"
section: "Higher-Order Functions"
params: [{ name: f, type: function }, { name: list, type: list }]
returns: "nil"
see_also: ["map", "tap", "list/times"]
---

Apply a function to each element purely for its side effects (printing, logging, mutating). Unlike `map`, it returns no useful value — use it when you care about *what the function does*, not what it produces.

```sema
(for-each println '("a" "b" "c"))
;; prints a, b, c (each on its own line)
```

See also `map`, which collects each call's return value into a new list. If you find yourself ignoring `map`'s result, reach for `for-each` instead — it signals intent and avoids building a throwaway list.

