---
name: "stream/to-bytes"
module: "streams"
section: "Extraction (Byte Buffers)"
---

Extract the accumulated contents of a byte-buffer stream as a bytevector.

```sema
(let ((s (stream/byte-buffer)))
  (stream/write s (bytevector 1 2 3))
  (stream/to-bytes s))   ;; => #u8(1 2 3)
```
