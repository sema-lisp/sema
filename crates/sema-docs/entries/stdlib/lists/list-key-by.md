---
name: "list/key-by"
module: "lists"
section: "Extraction"
params: [{ name: f, type: function }, { name: seq, type: list }]
returns: "map"
see_also: ["list/group-by", "map/from-entries", "list/pluck"]
---

Index a list into a map, keyed by the result of calling the function on each element. The value is the element itself — perfect for building an id-to-record lookup.

This assumes keys are **unique**: if two elements produce the same key, the later one wins (overwrites). When a key can repeat and you want all matches, use `list/group-by` (which keeps lists of elements).

```sema
(list/key-by (fn (p) (get p :id))
             (list {:id 1 :name "Alice"} {:id 2 :name "Bob"}))
; => {1 {:id 1 :name "Alice"} 2 {:id 2 :name "Bob"}}
```
