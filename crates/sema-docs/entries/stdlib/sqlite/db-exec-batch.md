---
name: "db/exec-batch"
module: "sqlite"
section: "Executing SQL"
---

Execute multiple SQL statements at once. STATIC SQL ONLY — there is no parameter binding, so the entire string is run verbatim. Useful for schema setup and migrations. Returns `nil`.

**Security:** never interpolate user-controlled input into the SQL string passed to `db/exec-batch` — doing so is a SQL injection vulnerability. For any value that comes from outside the program, use the parameterized `db/exec` (with `?` placeholders) instead, one statement at a time.

```sema
(db/exec-batch "mydb" "
  CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT);
  CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT);
  CREATE INDEX idx_posts_user ON posts(user_id);
")
```
