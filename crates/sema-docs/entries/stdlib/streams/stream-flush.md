---
name: "stream/flush"
module: "streams"
section: "Control"
params: [{ name: stream, type: stream }]
returns: "nil"
see_also: ["stream/write", "stream/write-string", "stream/close"]
---

Flush any buffered output to the underlying sink.

```sema
(stream/flush s)
```
