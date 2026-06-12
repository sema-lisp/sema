---
name: "stream/byte-buffer"
module: "streams"
section: "Creating Streams"
---

Create a read/write in-memory buffer. Writes append to the buffer; reads consume from the current position.

```sema
(define buf (stream/byte-buffer))
(stream/write buf (string->utf8 "hello"))
(stream/to-string buf)  ;; => "hello"
```
