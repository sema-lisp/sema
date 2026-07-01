---
name: "patch/apply-file"
module: "diff"
section: "Diff & Patch"
syntax: "(patch/apply-file path patch)"
returns: "int"
---

Read the file at `path`, apply the unified-diff `patch` to its contents (using the same logic as `diff/apply`), write the patched text back to the file, and return the number of hunks applied. Raises an error if any hunk fails to apply, in which case the file is left unmodified. Requires the `fs-write` capability.

```sema
(patch/apply-file "notes.txt" my-patch)
```
