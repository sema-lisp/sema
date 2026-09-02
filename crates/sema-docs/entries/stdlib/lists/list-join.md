---
name: "list/join"
module: "lists"
section: "Padding & Joining"
params: [{ name: seq, type: list }, { name: sep, type: string }, { name: final-sep, type: string, doc: "optional; defaults to sep" }]
returns: "string"
see_also: ["string/join", "interpose", "string/split"]
---

Join list elements into a string. Optional final separator.

```sema
(list/join '(1 2 3) ", ")             ; => "1, 2, 3"
(list/join '(1 2 3) ", " " and ")     ; => "1, 2 and 3"
```
