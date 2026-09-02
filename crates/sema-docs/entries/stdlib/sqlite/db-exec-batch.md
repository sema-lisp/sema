---
name: "db/exec-batch"
module: "sqlite"
section: "Executing SQL"
params: [{ name: db, type: string }, { name: sql, type: string }]
returns: "nil"
see_also: ["db/exec", "db/tables", "db/open"]
---

Execute several SQL statements in one call, separated by semicolons. STATIC SQL ONLY — there is **no** parameter binding, so the whole string runs verbatim. This is the right tool for schema setup and migrations (many `CREATE`/`ALTER`/`CREATE INDEX` in one shot). Returns `nil`.

**Security:** never interpolate user-controlled input into the SQL string passed to `db/exec-batch` — doing so is a SQL injection vulnerability. For any value that comes from outside the program, use the parameterized `db/exec` (with `?` placeholders) instead, one statement at a time.

Concurrent calls against the same handle serialize: inside `async/spawn`, a call queues automatically behind any other `db/*` call already in flight on that handle instead of racing the connection.

```sema
(db/exec-batch "mydb" "
  CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT);
  CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT);
  CREATE INDEX idx_posts_user ON posts(user_id);
")
; => nil
```
