---
name: "db/close"
module: "sqlite"
section: "Opening & Closing"
params: [{ name: db, type: string }]
returns: "nil"
see_also: ["db/open", "db/open-memory"]
---

Close a database connection and release its handle. Returns `nil`. After closing, the handle is invalid and further `db/*` calls on it will fail. For file databases this also flushes the WAL; for an in-memory database (`db/open-memory`) all data is discarded.

```sema
(db/close "mydb")   ; => nil
```
