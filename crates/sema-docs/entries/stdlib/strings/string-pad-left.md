---
name: "string/pad-left"
module: "strings"
section: "Core String Operations"
syntax: "(string/pad-left s width [pad-char])"
returns: "string"
---

Pad a string on the left to a given width.

```sema
(string/pad-left "42" 5 "0")   ; => "00042"
(string/pad-left "hi" 5)       ; => "   hi"
```
