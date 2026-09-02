---
name: "git/diff-files"
module: "git"
section: "Git"
returns: "list"
params: []
see_also: ["git/diff", "git/changed-files", "git/status"]
syntax: "(git/diff-files)"
---

Return a list of path strings for files that have unstaged changes relative to the index. Read-only — runs `git diff --name-only`.

```sema
(git/diff-files)  ; => ["src/main.sema"]
```
