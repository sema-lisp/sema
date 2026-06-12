---
name: "regex/match?"
module: "regex"
section: "Matching"
---

Test if a pattern matches anywhere in a string. Returns `#t` or `#f`.

```sema
(regex/match? #"\d+" "abc123")       ; => #t
(regex/match? #"\d+" "no digits")    ; => #f
(regex/match? #"^\d+$" "abc123")     ; => #f  (anchored — must match entire string)
(regex/match? #"^\d+$" "123")        ; => #t
```
