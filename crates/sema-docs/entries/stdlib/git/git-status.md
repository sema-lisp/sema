---
name: "git/status"
module: "git"
section: "Git"
returns: "list"
---

Return a list of maps describing the working-tree status, one per changed file. Each map has `:path` (the file path), `:status` (the two-character porcelain code), `:staged` (true when the index column carries a change), and `:untracked` (true for `"??"` entries). Read-only — runs `git status --porcelain=v1`.

```sema
(git/status)
; => [{:path "src/main.sema" :status " M" :staged false :untracked false}]
```
