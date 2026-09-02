---
name: "string/reverse"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string/chars", "list->string", "reverse"]
---

Reverse a string. Reverses by Unicode scalar character, so accented letters and emoji stay intact (it does not split a multi-byte character).

```sema
(string/reverse "hello")   ; => "olleh"
(string/reverse "héllo")   ; => "olléh"
```
