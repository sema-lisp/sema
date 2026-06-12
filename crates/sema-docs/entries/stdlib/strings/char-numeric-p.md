---
name: "char/numeric?"
module: "strings"
section: "Characters"
aliases: ["char-numeric?"]
---

Test if a character is numeric.

```sema
(char/numeric? #\5)      ; => #t
(char/numeric? #\a)      ; => #f
```
