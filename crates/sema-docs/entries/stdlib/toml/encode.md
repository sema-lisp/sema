---
name: "toml/encode"
module: "toml"
params: [{ name: m, type: map }]
returns: "string"
see_also: ["toml/decode", "json/encode"]
---

Serialize a map to a TOML document string. The top-level value must be a map; keys are emitted as TOML keys and values are converted to their TOML equivalents.

```sema
(toml/encode {:name "sema" :version 2})
; => "name = \"sema\"\nversion = 2\n"

;; nested maps become [section] tables
(toml/encode {:server {:host "localhost" :port 8080}})
; => "[server]\nhost = \"localhost\"\nport = 8080\n"
```

Inverse of `toml/decode`.
