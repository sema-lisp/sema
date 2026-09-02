---
name: "fs/unwatch"
module: "fs"
section: "File Watching"
params: [{ name: handle, type: int }]
returns: "nil"
see_also: ["fs/watch", "fs/watch-events"]
---

Stop watching and free the watcher handle created by `fs/watch`. The background thread is signalled to stop, the event queue is dropped, and the handle becomes invalid for `fs/watch-events`. Takes the integer handle and returns `nil`; calling it on an unknown or already-released handle is a no-op.

```sema
(define w (fs/watch "src"))
(fs/unwatch w)
```
