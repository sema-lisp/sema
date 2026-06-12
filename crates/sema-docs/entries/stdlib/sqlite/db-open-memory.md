---
name: "db/open-memory"
module: "sqlite"
section: "Opening & Closing"
---

Open an in-memory SQLite database. Useful for tests, temporary data, and caching.

```sema
(db/open-memory)           ; handle is ":memory:"
(db/open-memory "testdb")  ; handle is "testdb"
```
