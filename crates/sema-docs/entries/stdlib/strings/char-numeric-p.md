---
name: "char/numeric?"
module: "strings"
section: "Characters"
aliases: ["char-numeric?"]
params: [{ name: c, type: char }]
returns: "bool"
see_also: ["char/alphabetic?", "char/whitespace?", "string/number?"]
---

Test if a character is numeric.

```sema
(char/numeric? #\5)      ; => #t
(char/numeric? #\a)      ; => #f
```
