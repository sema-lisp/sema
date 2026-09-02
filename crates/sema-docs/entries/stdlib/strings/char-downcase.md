---
name: "char/downcase"
module: "strings"
section: "Characters"
aliases: ["char-downcase"]
params: [{ name: c, type: char }]
returns: "char"
see_also: ["char/upcase", "string/lower", "char-lower-case?"]
---

Convert a character to lowercase.

```sema
(char/downcase #\Z)   ; => #\z
```
