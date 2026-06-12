---
name: "string/chop-end"
module: "strings"
section: "Prefix & Suffix"
---

Remove a suffix if present.

```sema
(string/chop-end "file.txt" ".txt")  ; => "file"
(string/chop-end "file.txt" ".md")   ; => "file.txt"
```
