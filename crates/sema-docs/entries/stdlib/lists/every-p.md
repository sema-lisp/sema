---
name: "every?"
module: "lists"
section: "Searching & Testing"
params: [{ name: pred, type: function }, { name: seq, type: "list | vector" }]
returns: bool
---

Return `#t` if `pred` returns truthy for every element of `seq` (and for the empty sequence). Alias of `every`.

```sema
(every? odd? '(1 3 5))   ; => #t
(every? odd? '(1 2 3))   ; => #f
```
