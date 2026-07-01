---
name: "first"
module: "lists"
section: "Construction & Access"
params: [{ name: lst, type: list, doc: "list or vector; nil if empty" }]
returns: "any"
---

Alias for `car`. Return the first element of a list or vector. Returns `nil` for empty sequences (making it a safe "index 0" accessor — unlike `nth`, which errors out of bounds).

```sema
(first '(1 2 3))   ; => 1
(first [1 2 3])    ; => 1
(first [])         ; => nil
```
