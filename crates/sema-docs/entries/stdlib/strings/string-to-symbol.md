---
name: "string/to-symbol"
module: "strings"
section: "Type Conversions"
aliases: ["string->symbol"]
params: [{ name: s, type: string }]
returns: "symbol"
see_also: ["symbol/to-string", "string/to-keyword", "string/intern"]
---

Convert a string to a symbol.

```sema
(string/to-symbol "foo")   ; => foo
```
