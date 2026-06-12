---
name: "db/query"
module: "sqlite"
section: "Querying"
---

Execute a SELECT query and return all results as a list of maps. Column names become keyword keys. Supports parameterized queries.

```sema
(db/query "mydb" "SELECT * FROM users")
; => ({:id 1 :name "Alice" :age 31})

(db/query "mydb" "SELECT name, age FROM users WHERE age > ?" 25)
; => ({:age 31 :name "Alice"})
```
