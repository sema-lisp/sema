---
name: "regex/replace-all"
module: "regex"
section: "Replacement"
---

Replace **all** matches of a pattern.

```sema
(regex/replace-all #"\d" "X" "a1b2")        ; => "aXbX"
(regex/replace-all #"\s+" " " "a  b  c")    ; => "a b c"
```
