---
name: "fs/watch"
module: "fs"
section: "File Watching"
---

Watch a path for changes and return a watcher handle. `(fs/watch "src" {:recursive true})`. The OS delivers events on a background thread; drain them with `fs/watch-events`.
