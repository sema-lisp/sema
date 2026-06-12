---
name: "db/last-insert-id"
module: "sqlite"
section: "Utility"
---

Return the rowid of the last inserted row.

```sema
(db/exec "mydb" "INSERT INTO users (name, age) VALUES (?, ?)" "Bob" 25)
(db/last-insert-id "mydb")
; => 2
```
