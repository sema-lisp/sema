---
name: "db/query-one"
module: "sqlite"
section: "Querying"
syntax: "(db/query-one db sql param ...)"
returns: "map or nil"
see_also: ["db/query", "db/exec", "db/last-insert-id"]
---

Run a SELECT and return only the **first** row as a map, or `nil` when no row matches. The convenient form for primary-key / unique lookups. For multiple rows use `db/query`.

Concurrent calls against the same handle serialize: inside `async/spawn`, a call queues automatically behind any other `db/*` call already in flight on that handle instead of racing the connection.

```sema
(db/query-one "mydb" "SELECT * FROM users WHERE name = ?" "Alice")
; => {:age 31 :id 1 :name "Alice"}

(db/query-one "mydb" "SELECT * FROM users WHERE name = ?" "Nobody")
; => nil
```
