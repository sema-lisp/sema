---
name: "file/append"
module: "file-io"
section: "File Operations"
params: [{ name: path, type: string }, { name: content, type: string }]
returns: "nil"
see_also: ["file/write", "file/write-lines", "file/read"]
---

Append a string to the end of a file, creating it if it does not exist. Existing content is preserved (unlike `file/write`, which overwrites).

Append does not add a newline for you — include `\n` yourself if you want one line per call, as when building up a log.

```sema
(file/append "log.txt" "new line\n")   ; add a line; file kept intact
```
