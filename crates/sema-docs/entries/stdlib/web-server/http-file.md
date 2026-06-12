---
name: "http/file"
module: "web-server"
section: "Response Helpers"
---

Return a file from disk with automatic MIME type detection. The file is read on the I/O thread (not the evaluator), so it handles binary files efficiently.

```sema
(http/file "public/index.html")
(http/file "data/report.pdf" "application/pdf")  ; explicit content type
```

The path is resolved relative to the current working directory. If the file doesn't exist, an error is raised. The MIME type is guessed from the file extension (e.g. `.html` → `text/html`, `.css` → `text/css`, `.js` → `application/javascript`).
