---
name: "string/words"
module: "strings"
section: "Case Conversion"
params: [{ name: s, type: string }]
returns: "list"
see_also: ["string/lines", "string/split", "string/headline"]
---

Split a string into words on whitespace, underscores, and camelCase humps. Punctuation that
isn't a separator stays attached to its word.

```sema
(string/words "hello_world")     ; => ("hello" "world")
(string/words "helloWorld")      ; => ("hello" "World")
(string/words "Hello World!")    ; => ("Hello" "World!")
```
