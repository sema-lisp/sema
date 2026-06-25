---
name: "string/length"
module: "strings"
section: "Scheme Compatibility Aliases"
aliases: ["string-length"]
params: [{ name: s, type: string }]
returns: "int"
---

Return the number of characters in a string.

```sema
(string/length "hello")   ; => 5
(string/length "")        ; => 0
(string/length "héllo")   ; => 5
(string/length "日本語")   ; => 3
```
