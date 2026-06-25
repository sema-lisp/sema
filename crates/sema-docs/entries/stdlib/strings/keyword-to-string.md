---
name: "keyword/to-string"
module: "strings"
section: "Type Conversions"
aliases: ["keyword->string"]
params: [{ name: k, type: keyword }]
returns: "string"
---

Convert a keyword to a string.

```sema
(keyword/to-string :name)   ; => "name"
```
