---
name: "integer/to-char"
module: "strings"
section: "Characters"
aliases: ["integer->char"]
---

Convert a Unicode code point to a character.

```sema
(integer/to-char 65)    ; => #\A
(integer/to-char 955)   ; => #\λ
```
