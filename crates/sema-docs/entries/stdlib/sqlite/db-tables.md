---
name: "db/tables"
module: "sqlite"
section: "Utility"
params: [{ name: db, type: string }]
returns: "list"
see_also: ["db/query", "db/exec-batch", "db/open"]
---

List the names of all user-created tables in the database as a list of strings, sorted. Internal SQLite bookkeeping tables (those named `sqlite_*`) are excluded. Useful for introspection and migration checks.

```sema
(db/tables "mydb")
; => ("posts" "tags" "users")
```
