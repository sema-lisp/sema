---
name: "string/chop-start"
module: "strings"
section: "Prefix & Suffix"
---

Remove a prefix if present, otherwise return unchanged.

```sema
(string/chop-start "Hello World" "Hello ")  ; => "World"
(string/chop-start "Hello" "Bye")           ; => "Hello"
```
