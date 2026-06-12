---
name: "stream/read-byte"
module: "streams"
section: "Reading"
---

Read a single byte. Returns an integer 0–255, or `nil` at EOF.

```sema
(stream/read-byte s)   ;; => 65 (or nil at EOF)
```
