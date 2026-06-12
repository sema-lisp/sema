---
name: "stream/read-line"
module: "streams"
section: "Reading"
---

Read until newline (`\n`), returning a string without the newline. Strips trailing `\r` for Windows line endings. Returns `nil` at EOF.

```sema
(stream/read-line s)   ;; => "first line" (or nil)
```
