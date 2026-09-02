---
name: "regex/split"
module: "regex"
section: "Splitting"
params: [{ name: pattern, type: string }, { name: text, type: string }]
returns: "list"
see_also: ["string/split", "regex/find-all", "string/lines"]
---

Split a string by a regex delimiter.

```sema
(regex/split #"," "a,b,c")           ; => ("a" "b" "c")
(regex/split #"\s+" "hello  world")  ; => ("hello" "world")
(regex/split #"[,;]" "a,b;c,d")     ; => ("a" "b" "c" "d")
```
