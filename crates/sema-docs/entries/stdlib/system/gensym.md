---
name: "gensym"
module: "system"
section: "Metaprogramming"
params: [{ name: prefix, type: string }]
returns: symbol
---

Generate a fresh, unique symbol. The optional `prefix` (default `"g"`) is prepended to a counter so each call yields a distinct name. Useful for hygienic macro expansion.

```sema
(symbol? (gensym))      ; => #t
(gensym "tmp")          ; => tmp<n> (a unique symbol)
```
