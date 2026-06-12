---
name: "string->float"
module: "strings"
section: "Type Conversions"
aliases: ["string/to-float"]
params: [{ name: x, type: "string | number" }]
returns: "float"
---

Parse a string as a float. Integers and floats are accepted directly and returned as a float. Raises an error when a string cannot be parsed.

```sema
(string->float "3.14")   ; => 3.14
(string->float "42")     ; => 42.0
(string->float 7)        ; => 7.0
```
