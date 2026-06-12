---
name: "stream/write"
module: "streams"
section: "Writing"
---

Write a bytevector. Returns the number of bytes written.

```sema
(stream/write s (bytevector 72 101 108 108 111))  ;; => 5
```
