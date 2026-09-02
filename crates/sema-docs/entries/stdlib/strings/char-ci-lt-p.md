---
name: "char-ci<?"
module: "strings"
section: "Character Comparison (R7RS)"
params: [{ name: a, type: char }, { name: b, type: char }]
returns: "bool"
see_also: ["char<?", "char-ci=?", "char-ci>?", "char-ci<=?"]
---

Case-insensitive character less-than (compares the lowercased code points).

```sema
(char-ci<? #\A #\b)   ; => #t
```
