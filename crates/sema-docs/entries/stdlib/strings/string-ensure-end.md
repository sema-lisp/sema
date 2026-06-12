---
name: "string/ensure-end"
module: "strings"
section: "Prefix & Suffix"
---

Ensure a string ends with a suffix.

```sema
(string/ensure-end "path" "/")   ; => "path/"
(string/ensure-end "path/" "/")  ; => "path/"
```
