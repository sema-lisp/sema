---
name: "regex/split"
module: "regex"
section: "Splitting"
---

Split a string by a regex delimiter.

```sema
(regex/split #"," "a,b,c")           ; => ("a" "b" "c")
(regex/split #"\s+" "hello  world")  ; => ("hello" "world")
(regex/split #"[,;]" "a,b;c,d")     ; => ("a" "b" "c" "d")
```
