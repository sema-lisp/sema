---
name: "string/chop-end"
module: "strings"
section: "Prefix & Suffix"
params: [{ name: s, type: string }, { name: suffix, type: string }]
returns: "string"
see_also: ["string/chop-start", "string/ensure-end", "string/ends-with?"]
---

Remove a suffix if present, otherwise return the string unchanged. The inverse of `string/ensure-end`.

```sema
(string/chop-end "file.txt" ".txt")  ; => "file"
(string/chop-end "file.txt" ".md")   ; => "file.txt"   (suffix absent: unchanged)
```
