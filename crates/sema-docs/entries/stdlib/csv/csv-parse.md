---
name: "csv/parse"
module: "csv"
section: "CSV"
---

Parse a CSV string into a list of lists (rows of fields). No header processing — every row is returned as-is.

**Signature:** `(csv/parse csv-string) → list`

```sema
(csv/parse "a,b\n1,2\n3,4")
; => (("a" "b") ("1" "2") ("3" "4"))
```

Quoted fields with commas and newlines are handled correctly:

```sema
(csv/parse "name,bio\n\"Ada\",\"Mathematician, writer\"\n")
; => (("name" "bio") ("Ada" "Mathematician, writer"))
```
