---
name: "gzip/compress"
module: "archive"
section: "Compression & Archives"
---

Compress bytes with gzip, returning a gzip-compressed bytevector. Accepts either a bytevector or a string (its UTF-8 bytes are compressed). Pairs with `gzip/decompress` for a lossless round-trip.

```sema
(gzip/decompress (gzip/compress (string->utf8 "hello")))
```
