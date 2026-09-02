---
name: "string/between"
module: "strings"
section: "Slicing & Extraction"
params: [{ name: s, type: string }, { name: open, type: string }, { name: close, type: string }]
returns: "string"
see_also: ["string/after", "string/before", "string/unwrap"]
---

Extract the portion between the first `open` delimiter and the first `close` delimiter that follows it. If `open` isn't found you get `""`; if `close` isn't found you get everything after `open`.

```sema
(string/between "[hello]" "[" "]")  ; => "hello"
(string/between "start:middle:end" "start:" ":end")  ; => "middle"
(string/between "a[x]b[y]" "[" "]") ; => "x"      (first match wins)
(string/between "no-brackets" "[" "]") ; => ""    (open not found)
```
