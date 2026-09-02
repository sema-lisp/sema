---
name: "char/to-integer"
module: "strings"
section: "Characters"
aliases: ["char->integer"]
params: [{ name: c, type: char }]
returns: "int"
see_also: ["integer/to-char", "string/codepoints", "char/to-string"]
---

Convert a character to its Unicode code point. The inverse is `integer/to-char`.

```sema
(char/to-integer #\A)   ; => 65
(char/to-integer #\a)   ; => 97

;; Code points are contiguous, so arithmetic on them works:
(integer/to-char (+ (char/to-integer #\A) 32))  ; => #\a   (uppercase → lowercase)
```
