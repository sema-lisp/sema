---
name: "file/read-bytes"
module: "file-io"
section: "Binary File I/O"
---

Read a file as a bytevector (binary data).

```sema
(file/read-bytes "image.png")   ; => #u8(137 80 78 71 ...)
```
