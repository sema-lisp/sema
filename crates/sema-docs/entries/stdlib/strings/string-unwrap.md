---
name: "string/unwrap"
module: "strings"
section: "Prefix & Suffix"
---

Remove surrounding delimiters if both present.

```sema
(string/unwrap "(hello)" "(" ")")  ; => "hello"
(string/unwrap "hello" "(" ")")    ; => "hello"
```
