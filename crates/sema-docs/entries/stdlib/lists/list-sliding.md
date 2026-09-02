---
name: "list/sliding"
module: "lists"
section: "Windowing"
params: [{ name: seq, type: list }, { name: n, type: int }, { name: step, type: int, doc: "optional; defaults to 1" }]
returns: "list"
see_also: ["list/chunk", "list/page", "zip"]
---

Create a sliding window over a list. Optional step parameter.

```sema
(list/sliding '(1 2 3 4 5) 2)     ; => ((1 2) (2 3) (3 4) (4 5))
(list/sliding '(1 2 3 4 5 6) 2 3) ; => ((1 2) (4 5))
```
