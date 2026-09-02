---
name: "stream/read"
module: "streams"
section: "Reading"
params: [{ name: stream, type: stream }, { name: n, type: int }]
returns: "bytevector"
see_also: ["stream/read-all", "stream/read-byte", "stream/read-line", "stream/write"]
---

Read up to `n` bytes, returning a bytevector. Returns fewer bytes at EOF.

On a file-backed stream, calling `stream/read`, `stream/write`, `stream/read-line`, `stream/flush`, or `stream/close` again on the SAME stream object while one is already in flight (e.g. from a sibling `async/spawn` task) queues rather than racing the underlying file. In-memory streams (`stream/from-string`, `stream/byte-buffer`, …) are always synchronous and never queue.

```sema
(stream/read s 1024)   ;; => bytevector (up to 1024 bytes)
```
