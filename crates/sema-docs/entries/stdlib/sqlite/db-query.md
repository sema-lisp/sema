---
name: "db/query"
module: "sqlite"
section: "Querying"
syntax: "(db/query db sql param ...)"
returns: "list"
see_also: ["db/query-one", "db/exec", "db/tables"]
---

Run a SELECT and return **all** matching rows as a list of maps, one map per row, with column names as keyword keys. Returns the empty list `()` when nothing matches. Use `db/query-one` when you only need the first row.

Pass parameters as `?` placeholders with trailing values (safe binding, no injection risk). Note each row map is an ordered map, so its keys print sorted alphabetically — not in the SELECT column order.

Concurrent calls against the same handle serialize: inside `async/spawn`, a call queues automatically behind any other `db/*` call already in flight on that handle instead of racing the connection.

```sema
(db/query "mydb" "SELECT * FROM users")
; => ({:age 31 :id 1 :name "Alice"})

(db/query "mydb" "SELECT name, age FROM users WHERE age > ?" 25)
; => ({:age 31 :name "Alice"})
```
