---
name: "serial/close"
module: "serial"
section: "Connection Lifecycle"
params: [{ name: handle, type: int }]
returns: "nil"
see_also: ["serial/open", "serial/list"]
---

```sema
(serial/close handle)
```

Close the port and free the handle. Subsequent calls with that handle raise `invalid handle`.
