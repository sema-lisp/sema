---
name: "stream/from-bytes"
module: "streams"
section: "Creating Streams"
---

Create a readable stream from a bytevector.

```sema
(define s (stream/from-bytes (bytevector 1 2 3)))
(stream/read-byte s)    ;; => 1
(stream/read-byte s)    ;; => 2
```
