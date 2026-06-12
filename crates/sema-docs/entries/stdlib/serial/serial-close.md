---
name: "serial/close"
module: "serial"
section: "Connection Lifecycle"
---

```sema
(serial/close handle)
```

Close the port and free the handle. Subsequent calls with that handle raise `invalid handle`.
