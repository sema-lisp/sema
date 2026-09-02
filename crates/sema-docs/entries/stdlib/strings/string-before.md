---
name: "string/before"
module: "strings"
section: "Slicing & Extraction"
params: [{ name: s, type: string }, { name: needle, type: string }]
returns: "string"
see_also: ["string/before-last", "string/after", "string/between"]
---

Everything before the first occurrence of a needle.

```sema
(string/before "hello@world.com" "@")  ; => "hello"
(string/before "no-match" "@")         ; => "no-match"
```
