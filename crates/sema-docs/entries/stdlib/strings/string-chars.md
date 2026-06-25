---
name: "string/chars"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "list"
---

Convert a string to a list of characters.

```sema
(string/chars "abc")   ; => (#\a #\b #\c)
```
