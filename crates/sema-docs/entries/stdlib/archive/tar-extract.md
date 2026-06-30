---
name: "tar/extract"
module: "archive"
section: "Compression & Archives"
---

Extract every entry of the tar archive at `tar-path` into `dest-dir`, creating directories as needed. Gzip compression is auto-detected by file extension or magic bytes. Entries whose paths would escape the destination are skipped to guard against path traversal. Returns the number of entries extracted. Requires the `fs-write` capability.

```sema
(tar/extract "bundle.tar.gz" "out-dir")
```
