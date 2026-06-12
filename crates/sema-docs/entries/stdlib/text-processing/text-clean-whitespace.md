---
name: "text/clean-whitespace"
module: "text-processing"
section: "Text Cleaning"
---

Collapse multiple whitespace characters (spaces, newlines, tabs) into single spaces.

```sema
(text/clean-whitespace "  hello   world  \n\n  foo  ")
; => "hello world foo"
```
