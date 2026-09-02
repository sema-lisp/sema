---
name: "list/group-by"
module: "lists"
section: "Grouping"
params: [{ name: f, type: function }, { name: seq, type: list }]
returns: "map"
see_also: ["list/key-by", "frequencies", "partition"]
---

Bucket elements into a map keyed by the result of calling the function on each one. Every element lands in exactly one bucket, and within each bucket the original order is preserved.

Use this when one key can map to **many** elements (the values are lists). If each key is unique and you want a single element per key instead, use `list/key-by`.

```sema
(list/group-by even? '(1 2 3 4 5))        ; => {#f (1 3 5) #t (2 4)}
(list/group-by (fn (n) (mod n 3)) (range 7))  ; => {0 (0 3 6) 1 (1 4) 2 (2 5)}
```
