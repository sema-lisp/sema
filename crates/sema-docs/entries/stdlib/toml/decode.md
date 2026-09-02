---
name: "toml/decode"
module: "toml"
params: [{ name: s, type: string }]
returns: "map"
see_also: ["toml/encode", "json/decode", "markdown/frontmatter"]
---

Parse a TOML document string into a map. Table keys become keywords; nested tables (`[section]`) become nested maps and TOML arrays become lists.

```sema
(toml/decode "name = \"sema\"\nversion = 2")
; => {:name "sema" :version 2}

;; a [section] header nests into a map under that key
(toml/decode "[server]\nhost = \"localhost\"\nport = 8080")
; => {:server {:host "localhost" :port 8080}}

;; arrays decode to lists
(toml/decode "ports = [80, 443]")
; => {:ports (80 443)}
```

Pairs with `toml/encode` for a round-trip. Common for reading `Cargo.toml`-style config: `(toml/decode (file/read "config.toml"))`.
