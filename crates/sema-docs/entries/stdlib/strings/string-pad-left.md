---
name: "string/pad-left"
module: "strings"
section: "Core String Operations"
syntax: "(string/pad-left s width [pad-char])"
returns: "string"
see_also: ["string/pad-right", "string/repeat", "string/width"]
---

Pad a string on the left (right-aligning the text) up to a target width. The pad string defaults to a space. If the input is already at or over the width, it's returned unchanged — padding never truncates. Use `string/pad-right` to left-align instead.

```sema
(string/pad-left "42" 5 "0")    ; => "00042"
(string/pad-left "hi" 5)        ; => "   hi"
(string/pad-left "toolong" 3)   ; => "toolong"   ; already wider, unchanged
```
