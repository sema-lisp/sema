---
name: "string/contains?"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }, { name: sub, type: string }]
returns: "bool"
---

Test if a string contains a substring.

```sema
(string/contains? "hello" "ell")   ; => #t
(string/contains? "hello" "xyz")   ; => #f
```
