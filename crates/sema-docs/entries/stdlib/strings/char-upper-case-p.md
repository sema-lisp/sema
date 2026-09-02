---
name: "char/upper-case?"
module: "strings"
section: "Characters"
aliases: ["char-upper-case?"]
params: [{ name: c, type: char }]
returns: "bool"
see_also: ["char-lower-case?", "char/upcase", "char/alphabetic?"]
---

Test if a character is uppercase.

```sema
(char/upper-case? #\A)   ; => #t
(char/upper-case? #\a)   ; => #f
```
