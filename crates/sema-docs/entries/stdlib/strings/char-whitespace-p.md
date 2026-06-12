---
name: "char/whitespace?"
module: "strings"
section: "Characters"
aliases: ["char-whitespace?"]
---

Test if a character is whitespace.

```sema
(char/whitespace? #\space)   ; => #t
(char/whitespace? #\a)       ; => #f
```
