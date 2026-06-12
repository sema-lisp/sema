---
name: "string/contains?"
module: "strings"
section: "Core String Operations"
---

Test if a string contains a substring.

```sema
(string/contains? "hello" "ell")   ; => #t
(string/contains? "hello" "xyz")   ; => #f
```
