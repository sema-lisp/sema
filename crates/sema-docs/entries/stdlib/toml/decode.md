---
name: "toml/decode"
module: "toml"
params: [{ name: s, type: string }]
returns: "map"
---

Parse a TOML document string into a map. Table keys become keywords; nested tables become nested maps and arrays become lists.

```sema
(toml/decode "name = \"sema\"\nversion = 2")
; => {:name "sema" :version 2}
```
