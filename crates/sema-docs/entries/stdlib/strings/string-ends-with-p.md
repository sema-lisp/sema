---
name: "string/ends-with?"
module: "strings"
section: "Core String Operations"
---

Test if a string ends with a suffix.

```sema
(string/ends-with? "hello" "lo")   ; => #t
(string/ends-with? "hello" "he")   ; => #f
```
