---
name: "string/append"
module: "strings"
section: "Scheme Compatibility Aliases"
aliases: ["string-append"]
syntax: "(string/append str ...)"
returns: "string"
see_also: ["str", "string/join", "string/repeat"]
---

Concatenate strings together. Non-string arguments are stringified (display form), so it doubles as a forgiving join — but `str` reads more clearly when you're mixing in numbers.

```sema
(string/append "hello" " " "world")   ; => "hello world"
(string/append "a" "b" "c")           ; => "abc"
(string/append "n=" 42)               ; => "n=42"   (42 stringified)
```
