---
name: "stream/open-input"
module: "streams"
section: "Creating Streams"
---

Open a file for reading. Returns a buffered input stream. Sandbox-gated (`FS_READ`).

```sema
(define s (stream/open-input "data.csv"))
(define contents (stream/read-all s))
(stream/close s)
```
