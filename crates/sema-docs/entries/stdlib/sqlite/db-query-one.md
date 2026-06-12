---
name: "db/query-one"
module: "sqlite"
section: "Querying"
---

Execute a SELECT query and return only the first row as a map, or `nil` if no rows match.

```sema
(db/query-one "mydb" "SELECT * FROM users WHERE name = ?" "Alice")
; => {:id 1 :name "Alice" :age 31}

(db/query-one "mydb" "SELECT * FROM users WHERE name = ?" "Nobody")
; => nil
```
