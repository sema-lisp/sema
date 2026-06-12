---
name: "some?"
module: "lists"
section: "Searching & Testing"
params: [{ name: pred, type: function }, { name: seq, type: "list | vector" }]
returns: bool
---

Return `#t` if `pred` returns truthy for at least one element of `seq`. Alias of `any` (and of `any?`).

```sema
(some? even? '(1 3 4))   ; => #t
(some? even? '(1 3 5))   ; => #f
```
