---
outline: [2, 3]
---

# Key-Value Store

Sema includes a persistent, JSON-backed key-value store for storing structured data across sessions. Data is automatically flushed to disk on every write.

::: tip Sandbox capabilities
The key-value store works without extra configuration in Sema's default mode.
In a sandboxed run, `kv/open`, `kv/set`, and `kv/delete` require `fs-write`
(`FS_WRITE`) and return `PermissionDenied` when it is denied. See the
[CLI sandbox documentation](/docs/cli#sandbox).
:::

## How It Works

**File path** — You control where data is stored via the second argument to `kv/open`. Relative paths resolve from the current working directory. The file is **not created until the first write** (`kv/set` or `kv/delete`).

**Store names** — The first argument to `kv/open` is a logical handle used to reference the store in subsequent calls. Store names are scoped to the current process. Opening the same name twice replaces the previous handle.

**Flushing** — Every `kv/set` and `kv/delete` rewrites the entire backing file immediately. `kv/close` also flushes. There is no separate manual flush — persistence is automatic.

**JSON format** — The backing file is pretty-printed JSON, so you can inspect or edit it with any text editor. If an existing file contains malformed JSON, `kv/open` raises an error.

**Supported value types:**

| Sema | JSON | Notes |
|------|------|-------|
| `nil` | `null` | |
| `#t` / `#f` | `true` / `false` | |
| Integers | number | |
| Floats | number | `NaN` and `Infinity` become `null` |
| Strings | string | |
| Lists | array | Recursive |
| Maps (keyword keys) | object | Keys become strings |

**Performance** — Each write rewrites the whole file. This is ideal for small-to-medium stores (config, caches, counters). For large datasets or high-frequency writes, consider using `file/write` directly.

## Functions

### `kv/open`

Open (or create) a named KV store backed by a JSON file. If the file exists, its contents are loaded. Returns the store name.

```sema
(kv/open "config" "/path/to/config.json")  ; => "config"
(kv/open "cache" "cache.json")             ; relative to CWD
```

If the file doesn't exist yet, no file is created — that happens on the first `kv/set`.

### `kv/get`

Get a value by key. Returns `nil` if the key doesn't exist.

```sema
(kv/get "config" "api-key")  ; => "sk-..." or nil
```

### `kv/set`

Set a key-value pair. The value is serialized as JSON. Returns the value. Flushes to disk immediately.

```sema
(kv/set "config" "api-key" "sk-...")
(kv/set "config" "retries" 3)
(kv/set "config" "tags" '("a" "b" "c"))
(kv/set "config" "user" {:name "Alice" :role "admin"})
```

### `kv/delete`

Delete a key. Returns `#t` if the key existed, `#f` otherwise. Flushes to disk immediately.

```sema
(kv/delete "config" "api-key")  ; => #t
(kv/delete "config" "api-key")  ; => #f (already deleted)
```

### `kv/keys`

List all keys in the store. Returns a list of strings.

```sema
(kv/keys "config")  ; => ("api-key" "retries" "tags")
```

### `kv/close`

Close a store, flushing data and freeing the handle. Returns `nil`.

```sema
(kv/close "config")
```

Data is safe even without calling `kv/close` (every write already flushes), but closing frees memory and releases the store name.

## Examples

### Basic usage

```sema
;; Create a persistent store for caching API results
(kv/open "cache" "api-cache.json")

;; Store some data
(kv/set "cache" "user:123" {:name "Alice" :email "alice@example.com"})
(kv/set "cache" "user:456" {:name "Bob" :email "bob@example.com"})

;; Retrieve it
(kv/get "cache" "user:123")
; => {:email "alice@example.com" :name "Alice"}

;; List keys
(kv/keys "cache")
; => ("user:123" "user:456")

;; Clean up
(kv/delete "cache" "user:123")
(kv/close "cache")
```

### Application configuration with defaults

```sema
(kv/open "config" "app-config.json")

;; Set defaults only if not already configured
(when (nil? (kv/get "config" "theme"))
  (kv/set "config" "theme" "dark"))

(when (nil? (kv/get "config" "max-retries"))
  (kv/set "config" "max-retries" 3))

;; Use config values
(def theme (kv/get "config" "theme"))
(println (string/append "Using theme: " theme))
```

On first run this creates `app-config.json` with defaults. On subsequent runs, existing values are preserved.

### Persistent run counter

```sema
(kv/open "stats" "run-stats.json")

;; Increment run count across sessions
(let ((runs (or (kv/get "stats" "run-count") 0)))
  (kv/set "stats" "run-count" (+ runs 1))
  (kv/set "stats" "last-run" (time/format (time/now) "%Y-%m-%d %H:%M:%S")))

(println (string/append "Run #" (string (kv/get "stats" "run-count"))))
(kv/close "stats")
```

### Structured data with maps and lists

```sema
(kv/open "contacts" "contacts.json")

(kv/set "contacts" "alice"
  {:name "Alice" :email "alice@example.com" :tags '("admin" "dev")})

(kv/set "contacts" "bob"
  {:name "Bob" :email "bob@example.com" :tags '("dev")})

;; Retrieve and destructure
(def alice (kv/get "contacts" "alice"))
(:name alice)   ; => "Alice"
(:tags alice)   ; => ("admin" "dev")

;; List all contacts
(for-each (fn (key) (println (:name (kv/get "contacts" key))))
          (kv/keys "contacts"))
(kv/close "contacts")
```

## Tips

- The backing file is human-readable JSON — you can inspect or hand-edit it between runs.
- Store names are just logical handles. Choose descriptive names like `"config"`, `"cache"`, or `"sessions"`.
- Use `kv/keys` with iteration for bulk operations like export or cleanup.
- For write-heavy workloads on large datasets, consider writing JSON directly with `file/write` to avoid rewriting the entire file on each operation.
