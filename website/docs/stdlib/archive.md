---
outline: [2, 3]
---

# Archives

Gzip, zip, and tar. File-touching functions require `FS_READ` / `FS_WRITE`
(see [System](/docs/stdlib/system)).

```sema
(gzip/compress (string->bytevector "hello"))   ; => gzip bytevector
(gzip/decompress bytes)
(zip/create "out.zip" '("a.txt" "b.txt"))      ; => entry count
(zip/extract "out.zip" "dest/")                ; zip-slip guarded
(zip/list "out.zip")
(tar/create "out.tar.gz" '("a.txt"))           ; .tar.gz/.tgz auto-gzips
(tar/extract "out.tar.gz" "dest/")             ; traversal + symlink guarded
```

Extraction refuses entries that would escape the destination (`..`, absolute
paths, traversal symlinks) and rejects two entries that map to the same target.
