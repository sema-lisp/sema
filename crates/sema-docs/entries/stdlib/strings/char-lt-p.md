---
name: "char<?"
module: "strings"
section: "Character Comparison (R7RS)"
params: [{ name: a, type: char }, { name: b, type: char }]
returns: "bool"
see_also: ["char-ci<?", "char=?", "char>?", "char<=?"]
---

Character less-than (by code point).

```sema
(char<? #\a #\b)   ; => #t
```
