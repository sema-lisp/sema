---
name: "tap"
module: "lists"
section: "Utility"
params: [{ name: value, type: any }, { name: f, type: function }]
returns: "any"
see_also: ["for-each", "->", "map"]
---

Run a side-effecting function on a value, then return the **original value** unchanged — whatever the function returns is discarded. The value comes first, the function second: `(tap value f)`.

This lets you splice logging or inspection into the middle of a thread-first (`->`) pipeline without breaking the data flow.

```sema
(tap 42 (fn (x) (println x)))   ; prints 42, returns 42

(-> (range 5)
    (tap (fn (xs) (println "got:" xs)))   ; logs (0 1 2 3 4), passes it through
    (reverse))                            ; => (4 3 2 1 0)
```
