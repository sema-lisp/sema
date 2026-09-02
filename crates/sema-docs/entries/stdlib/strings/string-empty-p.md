---
name: "string/empty?"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "bool"
see_also: ["string/length", "string/trim", "empty?"]
---

Test if a string is empty.

```sema
(string/empty? "")      ; => #t
(string/empty? "hello") ; => #f
```
