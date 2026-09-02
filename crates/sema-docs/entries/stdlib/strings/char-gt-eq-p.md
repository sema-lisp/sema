---
name: "char>=?"
module: "strings"
section: "Character Comparison (R7RS)"
params: [{ name: a, type: char }, { name: b, type: char }]
returns: "bool"
see_also: ["char-ci>=?", "char=?", "char<?", "char>?"]
---

Character greater-than-or-equal.

```sema
(char>=? #\b #\a)   ; => #t
```
