---
name: "db/exec-batch"
module: "sqlite"
section: "Executing SQL"
---

Execute multiple SQL statements at once (no parameter binding). Useful for schema setup and migrations. Returns `nil`.

```sema
(db/exec-batch "mydb" "
  CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT);
  CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT);
  CREATE INDEX idx_posts_user ON posts(user_id);
")
```
