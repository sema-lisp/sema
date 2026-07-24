---
name: "stream/copy"
module: "streams"
section: "Control"
---

Copy bytes from a readable stream to a writable one and return the total copied.
The optional `max-bytes` limit defaults to 256 MiB. Every path rejects the
first over-limit chunk before writing it.

Inside the cooperative runtime, a copy with one file-backed side offloads that
resource. Stdin is polled cooperatively, so an open pipe remains cancellable.
File-to-file copy fails promptly because a safe one-call implementation requires
ordered acquisition of both resource gates. Copy files with bounded
`stream/read`/`stream/write` chunks instead.

```sema
;; Drain a string source into an in-memory buffer
(let ((in (stream/from-string "hello"))
      (out (stream/byte-buffer)))
  (stream/copy in out 1024)) ;; => 5
```
