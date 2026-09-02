---
name: "fs/watch"
module: "fs"
section: "File Watching"
params: [{ name: path, type: string }, { name: opts, type: map, doc: "optional; :recursive (default #t)" }]
returns: "int"
see_also: ["fs/watch-events", "fs/unwatch", "file/exists?"]
---

Watch a path for changes and return an integer watcher handle. `path` must exist; a missing path raises an error at call time. The optional `opts` map takes one key, `:recursive` (default `#t`), which controls whether subdirectories are watched as well. The OS delivers events on a background thread into a bounded queue; drain them with `fs/watch-events` and release the watcher with `fs/unwatch`. Requires the `fs-read` capability, and the path must be inside the sandbox's allowed roots.

```sema
(define w (fs/watch "src" {:recursive #f}))
(fs/watch-events w)
(fs/unwatch w)
```
