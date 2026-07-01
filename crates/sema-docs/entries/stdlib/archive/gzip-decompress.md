---
name: "gzip/decompress"
module: "archive"
section: "Compression & Archives"
---

Decompress a gzip-compressed bytevector, returning the original bytes as a bytevector. Errors if the input is not valid gzip data. Inverse of `gzip/compress`.

```sema
(utf8->string (gzip/decompress (gzip/compress (string->utf8 "hello"))))
```
