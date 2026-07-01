---
name: "git/diff-files"
module: "git"
section: "Git"
returns: "list"
---

Return a list of path strings for files that have unstaged changes relative to the index. Read-only — runs `git diff --name-only`.

```sema
(git/diff-files)  ; => ["src/main.sema"]
```
