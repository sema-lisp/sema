---
name: "char-ci>=?"
module: "strings"
section: "Character Comparison (R7RS)"
params: [{ name: a, type: char }, { name: b, type: char }]
returns: "bool"
---

Case-insensitive character greater-than-or-equal (compares the lowercased code points).

```sema
(char-ci>=? #\B #\b)   ; => #t
```
