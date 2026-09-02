---
name: "file/read-bytes"
module: "file-io"
section: "Binary File I/O"
params: [{ name: path, type: string }]
returns: "bytevector"
see_also: ["file/write-bytes", "file/read", "file/fold-lines-bytes"]
---

Read a file's raw bytes as a bytevector. Use this (not `file/read`) for binary data — images, archives, anything that isn't valid UTF-8 text. Pair with `file/write-bytes` to round-trip.

```sema
(file/read-bytes "image.png")   ; => #u8(137 80 78 71 ...)  (PNG magic bytes)
```
