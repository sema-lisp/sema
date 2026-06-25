---
name: "string/split"
module: "strings"
section: "Core String Operations"
aliases: ["string-split"]
params: [{ name: s, type: string }, { name: sep, type: string }]
returns: "list"
---

Split a string by a delimiter.

```sema
(string/split "a,b,c" ",")        ; => ("a" "b" "c")
(string/split "hello world" " ")  ; => ("hello" "world")
```
