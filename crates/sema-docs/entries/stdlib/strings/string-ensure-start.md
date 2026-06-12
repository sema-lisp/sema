---
name: "string/ensure-start"
module: "strings"
section: "Prefix & Suffix"
---

Ensure a string starts with a prefix (adds it if missing).

```sema
(string/ensure-start "/path" "/")   ; => "/path"
(string/ensure-start "path" "/")    ; => "/path"
```
