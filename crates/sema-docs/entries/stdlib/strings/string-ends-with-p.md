---
name: "string/ends-with?"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }, { name: suffix, type: string }]
returns: "bool"
see_also: ["string/starts-with?", "string/contains?", "string/chop-end"]
---

Test if a string ends with a suffix.

```sema
(string/ends-with? "hello" "lo")   ; => #t
(string/ends-with? "hello" "he")   ; => #f
```
