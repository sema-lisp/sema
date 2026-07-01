---
outline: [2, 3]
---

# Git (read-only)

Read-only helpers over the `git` binary — they never mutate the repository. All
require the `PROCESS` capability (see [System](/docs/stdlib/system)).

```sema
(git/root)                 ; repo toplevel
(git/current-branch)
(git/status)               ; list of {:path :status :staged :untracked}
(git/changed-files)        ; list of paths
(git/diff-files)           ; paths with unstaged changes (git diff --name-only)
(git/diff)                 ; or (git/diff "path") — unified diff
(git/recent-files 20)      ; files touched by the last N commits
(git/ignore-matches? "target/x")   ; => #t
```

Paths are returned as real UTF-8 (quoting disabled), and renames / paths with
spaces are parsed unambiguously via NUL-delimited porcelain.
