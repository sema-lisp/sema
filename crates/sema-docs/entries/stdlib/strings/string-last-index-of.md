---
name: "string/last-index-of"
module: "strings"
section: "Core String Operations"
---

Find the last occurrence of a substring. Returns the character index or `nil` if not found.

```sema
(string/last-index-of "abcabc" "abc")   ; => 3
(string/last-index-of "hello" "xyz")    ; => nil
```
