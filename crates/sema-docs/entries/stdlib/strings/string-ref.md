---
name: "string/ref"
module: "strings"
section: "Scheme Compatibility Aliases"
aliases: ["string-ref"]
params: [{ name: s, type: string }, { name: index, type: int }]
returns: "char"
---

Return the character at a given index.

```sema
(string/ref "hello" 0)    ; => #\h
(string/ref "hello" 4)    ; => #\o
```
