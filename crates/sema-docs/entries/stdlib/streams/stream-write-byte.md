---
name: "stream/write-byte"
module: "streams"
section: "Writing"
params: [{ name: stream, type: stream }, { name: byte, type: int }]
returns: "nil"
see_also: ["stream/read-byte", "stream/write", "stream/flush"]
---

Write a single byte (integer 0–255).

```sema
(stream/write-byte s 10)   ; write a newline
```
