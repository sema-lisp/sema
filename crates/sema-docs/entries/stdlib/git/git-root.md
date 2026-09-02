---
name: "git/root"
module: "git"
section: "Git"
returns: "string"
params: []
see_also: ["git/current-branch", "git/status", "path/canonicalize"]
syntax: "(git/root)"
---

Return the absolute path to the root of the current Git repository (the working tree's top level). Errors if the current directory is not inside a Git repository. Read-only — runs `git rev-parse --show-toplevel`.

```sema
(git/root)  ; => "/Users/you/projects/sema"
```
