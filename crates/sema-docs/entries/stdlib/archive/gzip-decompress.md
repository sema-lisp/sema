---
name: "gzip/decompress"
module: "archive"
section: "Compression & Archives"
params: [{ name: bytes, type: bytevector }]
returns: "bytevector"
see_also: ["gzip/compress", "file/read-bytes", "bytes/->string"]
---

Decompress a gzip-compressed bytevector, returning the original bytes as a bytevector. Errors if the input is not valid gzip data. Inverse of `gzip/compress`.

```sema
(utf8->string (gzip/decompress (gzip/compress (string->utf8 "hello"))))
```
