---
name: "toml/encode"
module: "toml"
params: [{ name: m, type: map }]
returns: "string"
---

Serialize a map to a TOML document string. The top-level value must be a map; keys are emitted as TOML keys and values are converted to their TOML equivalents.

```sema
(toml/encode {:name "sema" :version 2})
; => "name = \"sema\"\nversion = 2\n"
```
