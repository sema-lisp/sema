---
name: "fs/watch-events"
module: "fs"
section: "File Watching"
---

Drain pending filesystem events for a watcher (non-blocking): a list of `{:kind :paths}` maps where `:kind` is `:create`/`:modify`/`:remove`/`:access`/`:other`.
