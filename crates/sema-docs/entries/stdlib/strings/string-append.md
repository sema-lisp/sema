---
name: "string/append"
module: "strings"
section: "Scheme Compatibility Aliases"
aliases: ["string-append"]
syntax: "(string/append str ...)"
returns: "string"
---

Concatenate strings together.

```sema
(string/append "hello" " " "world")   ; => "hello world"
(string/append "a" "b" "c")           ; => "abc"
```
