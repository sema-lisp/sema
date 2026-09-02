---
name: "list/interleave"
module: "lists"
section: "Grouping"
syntax: "(list/interleave seq1 seq2 ...)"
returns: "list"
see_also: ["zip", "interpose", "list/cross-join"]
---

Interleave elements from two lists.

```sema
(list/interleave '(1 2 3) '(a b c))   ; => (1 a 2 b 3 c)
```
