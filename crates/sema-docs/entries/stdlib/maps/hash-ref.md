---
name: "hash-ref"
module: "maps"
section: "Access"
params: [{ name: m, type: map }, { name: key, type: any }, { name: default, type: any }]
returns: "any"
see_also: ["get", "get-in", "contains?"]
---

Look up `key` in map `m`, returning the associated value or `default` (`nil` if omitted) when the key is missing. Alias of `get` — prefer `get` in new code; `hash-ref` exists for Racket-style familiarity.

```sema
(hash-ref {:a 1 :b 2} :a)         ; => 1
(hash-ref {:a 1} :missing)        ; => nil
(hash-ref {:a 1} :missing "none") ; => "none"
```
