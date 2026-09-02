---
name: "stream/close"
module: "streams"
section: "Control"
params: [{ name: stream, type: stream }]
returns: "nil"
see_also: ["with-open", "stream/flush", "stream/open-input", "stream/open-output"]
---

Close a stream, releasing the underlying resource. Double-close is a no-op.

```sema
(stream/close s)
(stream/close s)   ; safe, does nothing
```
