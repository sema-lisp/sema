---
name: "file/write-bytes"
module: "file-io"
section: "Binary File I/O"
params: [{ name: path, type: string }, { name: bytes, type: bytevector }]
returns: "nil"
see_also: ["file/read-bytes", "file/write"]
---

Write a bytevector to a file as raw bytes, overwriting any existing content. The inverse of `file/read-bytes`.

Use this (not `file/write`) for binary data — images, compressed blobs, anything that isn't valid UTF-8 text.

```sema
(file/write-bytes "output.bin" #u8(1 2 3))   ; writes 3 raw bytes
(file/read-bytes "output.bin")               ; => #u8(1 2 3)
```
