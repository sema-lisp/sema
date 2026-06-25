---
name: "number/to-string"
module: "strings"
section: "Type Conversions"
aliases: ["number->string"]
params: [{ name: n, type: number }]
returns: "string"
---

Convert a number to a string.

```sema
(number/to-string 42)      ; => "42"
(number/to-string 3.14)   ; => "3.14"
```
