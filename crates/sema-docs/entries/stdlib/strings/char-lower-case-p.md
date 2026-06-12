---
name: "char-lower-case?"
module: "strings"
section: "Characters"
aliases: ["char/lower-case?"]
params: [{ name: c, type: char }]
returns: "bool"
---

Test if a character is lowercase.

```sema
(char-lower-case? #\a)   ; => #t
(char-lower-case? #\A)   ; => #f
```
