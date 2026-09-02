---
name: "string/ensure-start"
module: "strings"
section: "Prefix & Suffix"
params: [{ name: s, type: string }, { name: prefix, type: string }]
returns: "string"
see_also: ["string/ensure-end", "string/chop-start", "string/starts-with?"]
---

Ensure a string starts with a prefix (adds it only if missing; idempotent). The inverse of `string/chop-start`.

```sema
(string/ensure-start "/path" "/")   ; => "/path"
(string/ensure-start "path" "/")    ; => "/path"
```
