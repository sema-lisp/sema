---
name: "diff/stat"
module: "diff"
section: "Diff & Patch"
syntax: "(diff/stat patch)"
returns: "map"
---

Summarize a unified-diff string, returning a map `{:added <int> :removed <int> :hunks <int>}`. Added and removed counts are the number of `+` and `-` body lines (the `+++`/`---` file headers are excluded), and `:hunks` counts the `@@` markers. Useful for reporting the size of a change without applying it.

```sema
(diff/stat (diff/unified "a\nb\nc\n" "a\nB\nc\n"))
```
