---
name: "list/drop-while"
module: "lists"
section: "Splitting"
params: [{ name: pred, type: function }, { name: seq, type: list }]
returns: "list"
see_also: ["drop-while", "list/take-while", "drop"]
---

Drop elements from the front while a predicate holds.

```sema
(list/drop-while (fn (x) (< x 4)) '(1 2 3 4 5))   ; => (4 5)
```
