---
name: "text/word-count"
module: "text-processing"
section: "Text Cleaning"
params: [{ name: text, type: string }]
returns: "integer"
see_also: ["string/words", "text/split-sentences", "string/length"]
---

Count words in text (split by whitespace).

```sema
(text/word-count "hello world foo bar")  ; => 4
```
