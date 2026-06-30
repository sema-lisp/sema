---
name: "git/diff"
module: "git"
section: "Git"
returns: "string"
---

Return the unified diff of unstaged changes as a string. With no argument it diffs the whole working tree; with one path argument it limits the diff to that path. Read-only — runs `git diff` or `git diff -- <path>`.

```sema
(git/diff)              ; whole-tree diff
(git/diff "src/main.sema")  ; diff for one path
```
