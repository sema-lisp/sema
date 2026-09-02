---
name: "file/rename"
module: "file-io"
section: "File Operations"
params: [{ name: from, type: string }, { name: to, type: string }]
returns: "nil"
see_also: ["file/copy", "file/delete"]
---

Rename or move a file (or directory) to a new path. Renaming and moving are the same operation: change the path and the file moves there.

The destination's parent directory must already exist, and an existing destination is overwritten. Unlike `file/copy`, this does not leave the original behind.

```sema
(file/rename "old.txt" "new.txt")        ; rename in place
(file/rename "draft.txt" "done/final.txt")  ; move into another dir
```
