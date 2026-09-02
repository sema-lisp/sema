---
name: "char/upcase"
module: "strings"
section: "Characters"
aliases: ["char-upcase"]
params: [{ name: c, type: char }]
returns: "char"
see_also: ["char/downcase", "string/upper", "char/upper-case?"]
---

Convert a character to uppercase.

```sema
(char/upcase #\a)   ; => #\A
```
