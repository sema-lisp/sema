---
name: "db/open"
module: "sqlite"
section: "Opening & Closing"
---

Open (or create) a SQLite database file. Returns a handle string for use in subsequent calls. Enables WAL journal mode and foreign keys automatically.

```sema
;; Open with path as handle
(db/open "mydata.db")  ; => "mydata.db"

;; Open with a named handle
(db/open "mydb" "/path/to/data.db")  ; => "mydb"
```
