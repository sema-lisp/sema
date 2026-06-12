---
name: "db/exec"
module: "sqlite"
section: "Executing SQL"
---

Execute a SQL statement that modifies data (INSERT, UPDATE, DELETE, CREATE TABLE, etc.). Returns the number of affected rows as an integer. Supports parameterized queries.

```sema
(db/exec "mydb" "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
; => 0

(db/exec "mydb" "INSERT INTO users (name, age) VALUES (?, ?)" "Alice" 30)
; => 1

(db/exec "mydb" "UPDATE users SET age = ? WHERE name = ?" 31 "Alice")
; => 1
```
