---
name: "string/pad-right"
module: "strings"
section: "Core String Operations"
syntax: "(string/pad-right s width [pad])"
returns: "string"
see_also: ["string/pad-left", "string/repeat", "string/width"]
---

Pad a string on the right (left-aligning the text) up to a target width. The pad string defaults to a space. If the input is already at or over the width, it's returned unchanged — padding never truncates. Use `string/pad-left` to right-align instead.

```sema
(string/pad-right "hi" 5)       ; => "hi   "
(string/pad-right "42" 5 "0")   ; => "42000"
(string/pad-right "toolong" 3)  ; => "toolong"   ; already wider, unchanged
```
