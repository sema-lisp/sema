---
name: "string/contains?"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }, { name: sub, type: string }]
returns: "bool"
see_also: ["string/starts-with?", "string/ends-with?", "string/index-of"]
---

Test if a string contains a substring.

```sema
(string/contains? "hello" "ell")   ; => #t
(string/contains? "hello" "xyz")   ; => #f
```
