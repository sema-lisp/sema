---
name: "git/ignore-matches?"
module: "git"
section: "Git"
returns: "bool"
---

Return true when `path` is excluded by the repository's gitignore rules, false otherwise. Read-only — runs `git check-ignore -q <path>` and inspects its exit code (0 = ignored, 1 = not ignored).

```sema
(git/ignore-matches? "target/debug")  ; => true
(git/ignore-matches? "Cargo.toml")    ; => false
```
