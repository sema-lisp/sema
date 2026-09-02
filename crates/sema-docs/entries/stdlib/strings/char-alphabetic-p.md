---
name: "char/alphabetic?"
module: "strings"
section: "Characters"
aliases: ["char-alphabetic?"]
params: [{ name: c, type: char }]
returns: "bool"
see_also: ["char/numeric?", "char/whitespace?", "char/upper-case?", "char-lower-case?"]
---

Test if a character is alphabetic.

```sema
(char/alphabetic? #\a)   ; => #t
(char/alphabetic? #\5)   ; => #f
```
