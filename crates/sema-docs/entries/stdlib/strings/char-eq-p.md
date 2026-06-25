---
name: "char=?"
module: "strings"
section: "Character Comparison (R7RS)"
params: [{ name: a, type: char }, { name: b, type: char }]
returns: "bool"
---

Character equality.

```sema
(char=? #\a #\a)   ; => #t
(char=? #\a #\b)   ; => #f
```
