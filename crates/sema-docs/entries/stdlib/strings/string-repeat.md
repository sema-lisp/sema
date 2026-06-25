---
name: "string/repeat"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }, { name: n, type: int, doc: "number of repetitions" }]
returns: "string"
---

Repeat a string N times.

```sema
(string/repeat "ab" 3)   ; => "ababab"
(string/repeat "-" 5)    ; => "-----"
```
