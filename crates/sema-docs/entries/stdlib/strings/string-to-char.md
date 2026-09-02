---
name: "string/to-char"
module: "strings"
section: "Characters"
aliases: ["string->char"]
params: [{ name: s, type: string }]
returns: "char"
see_also: ["char/to-string", "string/to-list", "string/ref"]
---

Convert a single-character string to a character value (`#\...`). The string must contain exactly one character — an empty or multi-character string raises. To turn a whole string into characters use `string/to-list`.

```sema
(string/to-char "a")   ; => #\a
(string/to-char "λ")   ; => #\λ
```
