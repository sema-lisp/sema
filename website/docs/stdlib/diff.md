---
outline: [2, 3]
---

# Diff & Patch

Produce, inspect, and apply unified diffs.

```sema
(define patch (diff/unified old-text new-text))   ; unified diff string
(diff/apply old-text patch)                        ; => new-text
(diff/stat patch)                                  ; => {:added :removed :hunks}
(diff/hunks patch)                                 ; list of hunk maps
(diff/parse patch)                                 ; structured representation
(patch/apply-file "src/main.rs" patch)             ; apply to a file in place
```

`diff/apply` tolerates small drift (context shifted by a few lines) and errors
rather than mis-applying when a hunk's context can't be found.
