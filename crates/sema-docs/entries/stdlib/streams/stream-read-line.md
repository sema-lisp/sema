---
name: "stream/read-line"
module: "streams"
section: "Reading"
params: [{ name: stream, type: stream }]
returns: "string or nil"
see_also: ["stream/read-all", "stream/read", "stream/write-string"]
---

Read until newline (`\n`), returning a string without the newline. Strips trailing `\r` for Windows line endings. Returns `nil` at EOF.

```sema
(stream/read-line s)   ;; => "first line" (or nil)
```
