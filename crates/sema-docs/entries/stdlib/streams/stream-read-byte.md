---
name: "stream/read-byte"
module: "streams"
section: "Reading"
params: [{ name: stream, type: stream }]
returns: "int or nil"
see_also: ["stream/write-byte", "stream/read", "stream/read-line"]
---

Read a single byte. Returns an integer 0–255, or `nil` at EOF.

```sema
(stream/read-byte s)   ;; => 65 (or nil at EOF)
```
