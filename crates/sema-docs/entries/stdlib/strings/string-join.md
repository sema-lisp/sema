---
name: "string/join"
module: "strings"
section: "Core String Operations"
aliases: ["string-join"]
params: [{ name: lst, type: list }, { name: sep, type: string }]
returns: "string"
see_also: ["string/split", "string/append", "str"]
---

Join a list into a single string, inserting the separator between elements. Non-string elements are converted to their display form, so you can join numbers directly. The inverse of `string/split`.

```sema
(string/join '("a" "b" "c") ", ")   ; => "a, b, c"
(string/join '("x" "y") "-")        ; => "x-y"
(string/join (list 1 2 3) "-")      ; => "1-2-3"   ; numbers stringified
```
