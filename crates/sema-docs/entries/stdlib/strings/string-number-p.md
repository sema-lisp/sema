---
name: "string/number?"
module: "strings"
section: "Core String Operations"
---

Test if a string represents a valid number.

```sema
(string/number? "42")      ; => #t
(string/number? "3.14")   ; => #t
(string/number? "hello")  ; => #f
```
