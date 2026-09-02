---
name: "git/changed-files"
module: "git"
section: "Git"
returns: "list"
params: []
see_also: ["git/status", "git/diff-files", "git/recent-files"]
syntax: "(git/changed-files)"
---

Return a list of path strings for every file that differs from the index or working tree (staged, modified, or untracked). Rename entries report the destination path. Read-only — parses `git status --porcelain=v1`.

```sema
(git/changed-files)  ; => ["src/main.sema" "README.md"]
```
