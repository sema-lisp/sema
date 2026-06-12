---
name: "any?"
module: "lists"
section: "Searching & Testing"
params: [{ name: pred, type: function }, { name: seq, type: "list | vector" }]
returns: bool
---

Return `#t` if `pred` returns truthy for at least one element of `seq`. Alias of `any` (and of `some?`).

```sema
(any? odd? '(2 4 5 6))   ; => #t
(any? odd? '(2 4 6))     ; => #f
```
