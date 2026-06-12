---
name: "kv/open"
module: "kv-store"
section: "Functions"
---

Open (or create) a named KV store backed by a JSON file. If the file exists, its contents are loaded. Returns the store name.

```sema
(kv/open "config" "/path/to/config.json")  ; => "config"
(kv/open "cache" "cache.json")             ; relative to CWD
```

If the file doesn't exist yet, no file is created — that happens on the first `kv/set`.
