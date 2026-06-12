---
name: "string/starts-with?"
module: "strings"
section: "Core String Operations"
---

Test if a string starts with a prefix.

```sema
(string/starts-with? "hello" "he")   ; => #t
(string/starts-with? "hello" "lo")   ; => #f
```
