---
name: "string/between"
module: "strings"
section: "Slicing & Extraction"
---

Extract the portion between two delimiters.

```sema
(string/between "[hello]" "[" "]")  ; => "hello"
(string/between "start:middle:end" "start:" ":end")  ; => "middle"
```
