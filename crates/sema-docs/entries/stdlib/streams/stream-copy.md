---
name: "stream/copy"
module: "streams"
section: "Control"
---

Copy all bytes from one stream to another. Returns total bytes copied.

```sema
(with-stream (in (stream/open-input "src.bin"))
  (with-stream (out (stream/open-output "dst.bin"))
    (stream/copy in out)))   ;; => bytes copied
```
