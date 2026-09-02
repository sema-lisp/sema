---
name: "symbol/to-string"
module: "strings"
section: "Type Conversions"
aliases: ["symbol->string"]
params: [{ name: sym, type: symbol }]
returns: "string"
see_also: ["string/to-symbol", "keyword/to-string", "str"]
---

Convert a symbol to a string.

```sema
(symbol/to-string 'foo)   ; => "foo"
```
