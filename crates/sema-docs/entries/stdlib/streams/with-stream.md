---
name: "with-stream"
module: "streams"
section: "Resource Management"
---

Macro that binds a stream, executes the body, and automatically closes the stream on exit — even if an error is thrown.

```sema
(with-stream (s (stream/open-input "data.txt"))
  (stream/read-all s))
;; s is closed here, even if read-all threw an error

;; Write to a file
(with-stream (out (stream/open-output "output.txt"))
  (stream/write-string out "line 1\n")
  (stream/write-string out "line 2\n"))
;; file is flushed and closed
```
