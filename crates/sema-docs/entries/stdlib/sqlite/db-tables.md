---
name: "db/tables"
module: "sqlite"
section: "Utility"
---

List all user-created tables in the database (excludes internal SQLite tables). Returns a list of strings.

```sema
(db/tables "mydb")
; => ("posts" "tags" "users")
```
