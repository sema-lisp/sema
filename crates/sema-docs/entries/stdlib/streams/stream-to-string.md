---
name: "stream/to-string"
module: "streams"
section: "Extraction (Byte Buffers)"
---

Extract the contents of a byte-buffer stream as a UTF-8 string.

```sema
(let ((s (stream/byte-buffer)))
  (stream/write-string s "hello")
  (stream/to-string s))   ;; => "hello"
```
