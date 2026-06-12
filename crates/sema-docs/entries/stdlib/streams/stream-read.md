---
name: "stream/read"
module: "streams"
section: "Reading"
---

Read up to `n` bytes, returning a bytevector. Returns fewer bytes at EOF.

```sema
(stream/read s 1024)   ;; => bytevector (up to 1024 bytes)
```
