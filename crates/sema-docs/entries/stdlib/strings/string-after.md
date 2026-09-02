---
name: "string/after"
module: "strings"
section: "Slicing & Extraction"
params: [{ name: s, type: string }, { name: needle, type: string }]
returns: "string"
see_also: ["string/after-last", "string/before", "string/between"]
---

Everything after the first occurrence of a needle. Returns the original string if needle not found.

```sema
(string/after "hello@world.com" "@")  ; => "world.com"
(string/after "no-match" "@")         ; => "no-match"
```
