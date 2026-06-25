---
name: "string/join"
module: "strings"
section: "Core String Operations"
aliases: ["string-join"]
params: [{ name: lst, type: list }, { name: sep, type: string }]
returns: "string"
---

Join a list of strings with a separator.

```sema
(string/join '("a" "b" "c") ", ")  ; => "a, b, c"
(string/join '("x" "y") "-")      ; => "x-y"
```
