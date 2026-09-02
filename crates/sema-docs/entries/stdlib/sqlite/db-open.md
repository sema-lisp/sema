---
name: "db/open"
module: "sqlite"
section: "Opening & Closing"
syntax: "(db/open [name] path)"
returns: "string"
see_also: ["db/open-memory", "db/close", "db/exec", "db/query"]
---

Open (or create) a SQLite database file and return a string **handle** that every other `db/*` call takes as its first argument. WAL journal mode and foreign-key enforcement are enabled automatically. Call `db/close` when done.

With one argument the path doubles as the handle; with two, you name the handle yourself (handy when juggling several connections). Opening the same handle again reuses the existing connection.

```sema
;; Path doubles as the handle:
(db/open "mydata.db")            ; => "mydata.db"

;; Explicit handle name + path:
(db/open "mydb" "/path/to/data.db")  ; => "mydb"
```
