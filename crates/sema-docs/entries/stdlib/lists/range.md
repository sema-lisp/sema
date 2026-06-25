---
name: "range"
module: "lists"
section: "Basic Operations"
syntax: "(range end) | (range start end) | (range start end step)"
returns: "list"
---

Generate a list of integers. With one argument, generates 0 to N-1. With two, generates from start to end-1.

```sema
(range 5)       ; => (0 1 2 3 4)
(range 1 5)     ; => (1 2 3 4)
```
