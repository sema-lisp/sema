---
name: "db/open-memory"
module: "sqlite"
section: "Opening & Closing"
params: [{ name: name, type: string, doc: "optional; handle name, default \":memory:\"" }]
returns: "string"
see_also: ["db/open", "db/close", "db/exec-batch"]
---

Open an in-memory SQLite database — fast, requires no file, and vanishes when closed (or when the program exits). Ideal for tests, scratch data, and caching. Returns a handle just like `db/open`.

Pass a name to address it later; without one the handle is the literal `":memory:"`.

```sema
(db/open-memory)           ; handle is ":memory:"
(db/open-memory "testdb")  ; handle is "testdb"
```
