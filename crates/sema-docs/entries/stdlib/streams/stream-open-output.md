---
name: "stream/open-output"
module: "streams"
section: "Creating Streams"
---

Open (or create) a file for writing. Returns a buffered output stream. Sandbox-gated (`FS_WRITE`).

```sema
(define s (stream/open-output "output.txt"))
(stream/write-string s "hello world\n")
(stream/close s)
```
