---
name: "string/lines"
module: "strings"
section: "Core String Operations"
params: [{ name: s, type: string }]
returns: "list"
see_also: ["string/split", "string/join", "string/words"]
---

Split a string into lines on `\n` or `\r\n` (Clojure `split-lines` semantics). A trailing newline does not produce a final empty line. Use `string/split` when you need to split on a literal separator.

```sema
(string/lines "a\nb\r\nc\n")   ; => ("a" "b" "c")
(string/lines "single")         ; => ("single")
```
