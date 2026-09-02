---
name: "regex/replace-all"
module: "regex"
section: "Replacement"
params: [{ name: pattern, type: string }, { name: replacement, type: string }, { name: text, type: string }]
returns: "string"
see_also: ["regex/replace", "string/replace", "regex/find-all"]
---

Replace **all** matches of a pattern.

```sema
(regex/replace-all #"\d" "X" "a1b2")        ; => "aXbX"
(regex/replace-all #"\s+" " " "a  b  c")    ; => "a b c"
```
