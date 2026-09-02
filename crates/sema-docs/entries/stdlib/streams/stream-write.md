---
name: "stream/write"
module: "streams"
section: "Writing"
params: [{ name: stream, type: stream }, { name: bytes, type: bytevector }]
returns: "int"
see_also: ["stream/write-string", "stream/write-byte", "stream/read", "stream/flush"]
---

Write a bytevector. Returns the number of bytes written.

```sema
(stream/write s (bytevector 72 101 108 108 111))  ;; => 5
```
