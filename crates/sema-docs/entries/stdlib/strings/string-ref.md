---
name: "string/ref"
module: "strings"
section: "Scheme Compatibility Aliases"
aliases: ["string-ref"]
params: [{ name: s, type: string }, { name: index, type: int }]
returns: "char"
see_also: ["string/slice", "string/length", "string/to-char"]
---

Return the character at a 0-based index, counting Unicode scalar characters (not bytes). An out-of-bounds index raises. For a range of characters use `string/slice`; to iterate all of them use `string/to-list`.

```sema
(string/ref "hello" 0)    ; => #\h
(string/ref "hello" 4)    ; => #\o
(string/ref "héllo" 1)    ; => #\é   ; character index, not byte
```
