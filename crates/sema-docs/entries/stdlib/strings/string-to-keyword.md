---
name: "string/to-keyword"
module: "strings"
section: "Type Conversions"
aliases: ["string->keyword"]
params: [{ name: s, type: string }]
returns: "keyword"
---

Convert a string to a keyword.

```sema
(string/to-keyword "name")   ; => :name
```
