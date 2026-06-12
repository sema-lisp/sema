---
name: "string/wrap"
module: "strings"
section: "Prefix & Suffix"
---

Wrap a string with left and right delimiters.

```sema
(string/wrap "hello" "(" ")")   ; => "(hello)"
(string/wrap "hello" "**")      ; => "**hello**"
```
