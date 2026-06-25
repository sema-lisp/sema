---
name: "string/to-number"
module: "strings"
section: "Type Conversions"
aliases: ["string->number"]
params: [{ name: s, type: string }]
returns: "number"
---

Parse a string as a number.

```sema
(string/to-number "42")     ; => 42
(string/to-number "3.14")  ; => 3.14
```
