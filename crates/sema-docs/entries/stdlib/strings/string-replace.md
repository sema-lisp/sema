---
name: "string/replace"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }, { name: from, type: string }, { name: to, type: string }]
returns: "string"
see_also: ["string/replace-first", "string/replace-last", "regex/replace-all", "string/remove"]
---

Replace every occurrence of a substring with a replacement. The match is a *literal* substring, not a regex — `.`, `*`, etc. match themselves. For pattern-based replacement use `regex/replace`; to replace only the first or last hit, use `string/replace-first` / `string/replace-last`.

```sema
(string/replace "hello" "l" "r")   ; => "herro"
(string/replace "aaa" "a" "b")     ; => "bbb"
(string/replace "a.b.c" "." "-")   ; => "a-b-c"   ; literal dot, not regex
```
