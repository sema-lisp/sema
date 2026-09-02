---
name: "fs/watch-events"
module: "fs"
section: "File Watching"
params: [{ name: handle, type: int }]
returns: "list"
see_also: ["fs/watch", "fs/unwatch"]
---

Drain the pending filesystem events for a watcher handle returned by `fs/watch`. The call does not block: it returns whatever the background thread has queued since the previous drain, or the empty list when nothing happened. Each event is a map `{:kind :paths}` where `:kind` is `:create`, `:modify`, `:remove`, `:access`, or `:other`, and `:paths` is a list of path strings. When the event queue overflowed between drains, one extra `{:kind :overflow}` map is appended, and some events were lost. An unknown handle raises an error.

```sema
(define w (fs/watch "src"))
(for-each (fn (e) (println (:kind e) (:paths e)))
          (fs/watch-events w))
```
