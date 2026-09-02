---
name: "char/whitespace?"
module: "strings"
section: "Characters"
aliases: ["char-whitespace?"]
params: [{ name: c, type: char }]
returns: "bool"
see_also: ["char/alphabetic?", "char/numeric?", "string/trim"]
---

Test if a character is whitespace.

```sema
(char/whitespace? #\space)   ; => #t
(char/whitespace? #\a)       ; => #f
```
