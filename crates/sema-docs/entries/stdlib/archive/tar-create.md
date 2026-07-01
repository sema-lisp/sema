---
name: "tar/create"
module: "archive"
section: "Compression & Archives"
---

Create a tar archive at `out-path` containing the given list of files, each added under its basename. The archive is gzip-compressed when `out-path` ends in `.tar.gz` or `.tgz`, otherwise it is an uncompressed tar. Returns the number of entries written. Requires the `fs-write` capability.

```sema
(tar/create "bundle.tar.gz" (list "a.txt" "b.txt"))
```
