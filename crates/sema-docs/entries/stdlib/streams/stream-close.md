---
name: "stream/close"
module: "streams"
section: "Control"
---

Close a stream, releasing the underlying resource. Double-close is a no-op.

```sema
(stream/close s)
(stream/close s)   ; safe, does nothing
```
