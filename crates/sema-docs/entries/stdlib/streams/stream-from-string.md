---
name: "stream/from-string"
module: "streams"
section: "Creating Streams"
---

Create a read-only stream from a string's UTF-8 bytes.

```sema
(define s (stream/from-string "hello world"))
(stream/read-byte s)    ;; => 104 (ASCII 'h')
(stream/read s 5)       ;; => #u8(101 108 108 111 32) ("ello ")
```
