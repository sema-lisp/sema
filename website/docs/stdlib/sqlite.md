---
outline: [2, 3]
---

# SQLite

Sema includes built-in SQLite support via the `db/*` functions, backed by [rusqlite](https://docs.rs/rusqlite). Databases are opened by name (a logical handle) and can be either file-backed or in-memory. WAL mode and foreign keys are enabled by default.

::: tip
`db/open` and `db/open-memory` require filesystem write capabilities (they are gated by `FS_WRITE`).
:::

## Opening & Closing

### `db/open`

Open (or create) a SQLite database file. Returns a handle string for use in subsequent calls. Enables WAL journal mode and foreign keys automatically.

```sema
;; Open with path as handle
(db/open "mydata.db")  ; => "mydata.db"

;; Open with a named handle
(db/open "mydb" "/path/to/data.db")  ; => "mydb"
```

### `db/open-memory`

Open an in-memory SQLite database. Useful for tests, temporary data, and caching.

```sema
(db/open-memory)           ; handle is ":memory:"
(db/open-memory "testdb")  ; handle is "testdb"
```

### `db/close`

Close a database connection and release the handle. Returns `nil`.

```sema
(db/close "mydb")
```

## Executing SQL

### `db/exec`

Execute a SQL statement that modifies data (INSERT, UPDATE, DELETE, CREATE TABLE, etc.). Returns the number of affected rows as an integer. Supports parameterized queries.

```sema
(db/exec "mydb" "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
; => 0

(db/exec "mydb" "INSERT INTO users (name, age) VALUES (?, ?)" "Alice" 30)
; => 1

(db/exec "mydb" "UPDATE users SET age = ? WHERE name = ?" 31 "Alice")
; => 1
```

### `db/exec-batch`

Execute multiple SQL statements at once. **Static SQL only** — there is no parameter binding, so the entire string is run verbatim. Useful for schema setup and migrations. Returns `nil`.

::: danger SQL injection
Never interpolate user-controlled input into the SQL string passed to `db/exec-batch` — doing so is a SQL injection vulnerability. For any value that comes from outside the program, use the parameterized [`db/exec`](#db-exec) (with `?` placeholders) instead, one statement at a time.
:::

```sema
(db/exec-batch "mydb" "
  CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT);
  CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT);
  CREATE INDEX idx_posts_user ON posts(user_id);
")
```

## Querying

### `db/query`

Execute a SELECT query and return all results as a list of maps. Column names become keyword keys. Supports parameterized queries.

```sema
(db/query "mydb" "SELECT * FROM users")
; => ({:id 1 :name "Alice" :age 31})

(db/query "mydb" "SELECT name, age FROM users WHERE age > ?" 25)
; => ({:age 31 :name "Alice"})
```

### `db/query-one`

Execute a SELECT query and return only the first row as a map, or `nil` if no rows match.

```sema
(db/query-one "mydb" "SELECT * FROM users WHERE name = ?" "Alice")
; => {:id 1 :name "Alice" :age 31}

(db/query-one "mydb" "SELECT * FROM users WHERE name = ?" "Nobody")
; => nil
```

## Utility

### `db/last-insert-id`

Return the rowid of the last inserted row.

```sema
(db/exec "mydb" "INSERT INTO users (name, age) VALUES (?, ?)" "Bob" 25)
(db/last-insert-id "mydb")
; => 2
```

### `db/tables`

List all user-created tables in the database (excludes internal SQLite tables). Returns a list of strings.

```sema
(db/tables "mydb")
; => ("posts" "tags" "users")
```

## Type Mapping

| Sema type   | SQLite type | Notes                        |
| ----------- | ----------- | ---------------------------- |
| `nil`       | NULL        |                              |
| Boolean     | INTEGER     | `#t` = 1, `#f` = 0          |
| Integer     | INTEGER     |                              |
| Float       | REAL        |                              |
| String      | TEXT        |                              |
| Bytevector  | BLOB        |                              |
| Other       | TEXT        | Converted via `to-string`    |

SQLite values map back as: NULL to `nil`, INTEGER to int, REAL to float, TEXT to string, BLOB to bytevector.

## Examples

### Basic CRUD

```sema
(db/open-memory "app")

(db/exec "app" "CREATE TABLE todos (id INTEGER PRIMARY KEY, task TEXT, done INTEGER DEFAULT 0)")

;; Insert
(db/exec "app" "INSERT INTO todos (task) VALUES (?)" "Buy groceries")
(db/exec "app" "INSERT INTO todos (task) VALUES (?)" "Write docs")

;; Query
(db/query "app" "SELECT * FROM todos WHERE done = 0")
; => ({:done 0 :id 1 :task "Buy groceries"} {:done 0 :id 2 :task "Write docs"})

;; Update
(db/exec "app" "UPDATE todos SET done = 1 WHERE id = ?" 1)

;; Delete
(db/exec "app" "DELETE FROM todos WHERE done = 1")

(db/close "app")
```

### Using with LLM extraction

```sema
(db/open-memory "contacts")
(db/exec "contacts" "CREATE TABLE people (name TEXT, email TEXT, company TEXT)")

;; Extract structured data from text and insert directly
(define info (llm/extract
  {:name {:type :string} :email {:type :string} :company {:type :string}}
  "Contact Alice at alice@acme.com, she works at Acme Corp"))

(db/exec "contacts" "INSERT INTO people (name, email, company) VALUES (?, ?, ?)"
  (:name info) (:email info) (:company info))

(db/query "contacts" "SELECT * FROM people")
; => ({:company "Acme Corp" :email "alice@acme.com" :name "Alice"})

(db/close "contacts")
```
