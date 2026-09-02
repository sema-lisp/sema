---
name: "db/last-insert-id"
module: "sqlite"
section: "Utility"
params: [{ name: db, type: string }]
returns: "int"
see_also: ["db/exec", "db/query-one"]
---

Return the rowid (integer primary key) of the most recent successful INSERT on this connection. Call it immediately after the insert to capture the generated id.

This reads an in-memory field with no I/O, so it never offloads or queues — if another `db/*` call on the same handle is mid-flight (e.g. a concurrent `async/spawn` offload), it fails immediately with a "database is busy" error rather than waiting for it.

```sema
(db/exec "mydb" "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)")
(db/exec "mydb" "INSERT INTO users (name) VALUES (?)" "Alice")
(db/last-insert-id "mydb")   ; => 1
```
