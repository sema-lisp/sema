---
name: "string/foldcase"
module: "strings"
section: "Unicode & Encoding"
params: [{ name: s, type: string }]
returns: "string"
see_also: ["string-ci=?", "string/lower", "string/normalize"]
---

Apply full Unicode case folding (CaseFolding.txt C+F mappings) to a string. Useful for case-insensitive comparisons and normalization.

Unlike `string/lower`, folding maps characters to a canonical caseless form rather than simply lowercasing: the German sharp s `ß` folds to `ss`, and final-sigma `ς` folds the same as medial `σ`. This is what makes `string/foldcase` the correct basis for caseless matching (see `string-ci=?`).

```sema
(string/foldcase "HELLO")        ; => "hello"
(string/foldcase "Hello World")  ; => "hello world"
(string/foldcase "Straße")       ; => "strasse"   ; string/lower leaves "straße"
(string/foldcase "ΩΜΕΓΑ")        ; => "ωμεγα"
```
