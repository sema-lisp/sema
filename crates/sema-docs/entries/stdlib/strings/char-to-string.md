---
name: "char/to-string"
module: "strings"
section: "Characters"
aliases: ["char->string"]
params: [{ name: c, type: char }]
returns: "string"
---

Convert a character to a single-character string.

```sema
(char/to-string #\a)   ; => "a"
```
