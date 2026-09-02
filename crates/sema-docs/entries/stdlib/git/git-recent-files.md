---
name: "git/recent-files"
module: "git"
section: "Git"
returns: "list"
params: [{ name: n, type: int, doc: "optional; number of commits, default 20" }]
see_also: ["git/changed-files", "git/diff-files", "git/status"]
---

Return a deduplicated list of file paths touched by the last `n` commits, preserving first-seen order. `n` defaults to 20 when omitted. Read-only — runs `git log --name-only --pretty=format: -n <n>`.

```sema
(git/recent-files)    ; files from the last 20 commits
(git/recent-files 5)  ; files from the last 5 commits
```
