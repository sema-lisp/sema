---
name: "with-open"
module: "streams"
section: "Resource Management"
---

Macro that binds a closeable resource, executes the body, and closes it on exit — even if an error is thrown. A RAII alias of `with-stream` that reads naturally for files and sockets.

```sema
(with-open (sock (ws/connect "wss://example.com"))
  (ws/send sock "hi")
  (ws/recv sock))
;; sock is closed here, even if the body threw
```
