---
name: "string/split"
module: "strings"
section: "Core String Operations"
aliases: ["string-split"]
params: [{ name: s, type: string }, { name: sep, type: string }]
returns: "list"
see_also: ["string/join", "string/lines", "regex/split"]
---

Split a string by a literal delimiter, returning a list of parts. Consecutive delimiters produce empty strings (parts are not coalesced), so the result length is always `(matches + 1)`. The inverse is `string/join`. To split on newlines specifically, prefer `string/lines` (it handles `\r\n` and trailing newlines).

```sema
(string/split "a,b,c" ",")        ; => ("a" "b" "c")
(string/split "hello world" " ")  ; => ("hello" "world")
(string/split "a,,b" ",")         ; => ("a" "" "b")   ; empty part between commas
```
