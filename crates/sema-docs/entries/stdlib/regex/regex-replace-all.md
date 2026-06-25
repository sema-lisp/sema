---
name: "regex/replace-all"
module: "regex"
section: "Replacement"
params: [{ name: pattern, type: string }, { name: replacement, type: string }, { name: text, type: string }]
returns: "string"
---

Replace **all** matches of a pattern.

```sema
(regex/replace-all #"\d" "X" "a1b2")        ; => "aXbX"
(regex/replace-all #"\s+" " " "a  b  c")    ; => "a b c"
```
