---
name: "char/to-integer"
module: "strings"
section: "Characters"
aliases: ["char->integer"]
params: [{ name: c, type: char }]
returns: "int"
---

Convert a character to its Unicode code point.

```sema
(char/to-integer #\A)   ; => 65
(char/to-integer #\a)   ; => 97
```
