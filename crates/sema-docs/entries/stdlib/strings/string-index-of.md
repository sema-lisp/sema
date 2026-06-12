---
name: "string/index-of"
module: "strings"
section: "Core String Operations"
---

Return the character index of the first occurrence of a substring, or `nil` if not found.

```sema
(string/index-of "hello" "ll")   ; => 2
(string/index-of "hello" "xyz")  ; => nil
```
