---
name: "string/slice"
module: "strings"
section: "Scheme Compatibility Aliases"
aliases: ["substring"]
---

Extract a substring by start and end character index.

```sema
(string/slice "hello" 1 3)   ; => "el"
(string/slice "hello" 0 5)   ; => "hello"
(string/slice "héllo" 1 2)   ; => "é"
```
