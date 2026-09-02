---
name: "string/to-keyword"
module: "strings"
section: "Type Conversions"
aliases: ["string->keyword"]
params: [{ name: s, type: string }]
returns: "keyword"
see_also: ["keyword/to-string", "string/to-symbol", "string/intern"]
---

Convert a string to a keyword. The inverse is `keyword->string`. The leading `:` is not part of the contents — don't include it in the input.

```sema
(string/to-keyword "name")                  ; => :name
(keyword->string (string/to-keyword "name")) ; => "name"
```
