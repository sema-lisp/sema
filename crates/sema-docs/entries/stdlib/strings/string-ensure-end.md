---
name: "string/ensure-end"
module: "strings"
section: "Prefix & Suffix"
params: [{ name: s, type: string }, { name: suffix, type: string }]
returns: "string"
see_also: ["string/ensure-start", "string/chop-end", "string/ends-with?"]
---

Ensure a string ends with a suffix, appending it only if it's not already there (idempotent). The inverse of `string/chop-end`.

```sema
(string/ensure-end "path" "/")   ; => "path/"
(string/ensure-end "path/" "/")  ; => "path/"   (already ends with /: unchanged)
```
