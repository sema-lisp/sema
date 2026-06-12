---
name: "char-ci>?"
module: "strings"
section: "Character Comparison (R7RS)"
params: [{ name: a, type: char }, { name: b, type: char }]
returns: "bool"
---

Case-insensitive character greater-than (compares the lowercased code points).

```sema
(char-ci>? #\B #\a)   ; => #t
```
