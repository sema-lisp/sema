---
name: "stream/to-bytes"
module: "streams"
section: "Extraction (Byte Buffers)"
params: [{ name: stream, type: stream }]
returns: "bytevector"
see_also: ["stream/to-string", "stream/byte-buffer", "stream/from-bytes"]
---

Extract the accumulated contents of a byte-buffer stream as a bytevector.

```sema
(let ((s (stream/byte-buffer)))
  (stream/write s (bytevector 1 2 3))
  (stream/to-bytes s))   ;; => #u8(1 2 3)
```
