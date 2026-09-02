---
name: "text/clean-whitespace"
module: "text-processing"
section: "Text Cleaning"
params: [{ name: text, type: string }]
returns: "string"
see_also: ["text/normalize-newlines", "text/trim-indent", "string/trim"]
---

Collapse multiple whitespace characters (spaces, newlines, tabs) into single spaces.

```sema
(text/clean-whitespace "  hello   world  \n\n  foo  ")
; => "hello world foo"
```
