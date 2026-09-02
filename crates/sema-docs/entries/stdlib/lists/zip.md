---
name: "zip"
module: "lists"
section: "Sublists"
syntax: "(zip seq1 seq2 ...)"
returns: "list"
see_also: ["list/interleave", "map", "enumerate", "list/cross-join", "map/zip"]
---

Combine corresponding elements from two lists into pairs.

```sema
(zip '(1 2 3) '("a" "b" "c"))   ; => ((1 "a") (2 "b") (3 "c"))
```
