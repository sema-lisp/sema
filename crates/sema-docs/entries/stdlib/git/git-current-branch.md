---
name: "git/current-branch"
module: "git"
section: "Git"
returns: "string"
params: []
see_also: ["git/root", "git/status"]
syntax: "(git/current-branch)"
---

Return the name of the currently checked-out branch (for example `"main"`). On a detached HEAD this returns `"HEAD"`. Read-only — runs `git rev-parse --abbrev-ref HEAD`.

```sema
(git/current-branch)  ; => "main"
```
