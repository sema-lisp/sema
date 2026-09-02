---
name: "term/with-bracketed-paste"
module: "terminal"
section: "Screen Control"
syntax: "(term/with-bracketed-paste body ...)"
returns: "any"
see_also: ["term/enable-bracketed-paste", "term/disable-bracketed-paste", "io/with-raw-mode"]
---

Guard macro: enable bracketed paste, run `body`, and **always** disable it on exit — even if `body` throws (the error is re-raised after restoring). Returns `body`'s value. Inside, `io/read-key` returns pasted text as `{:kind :paste :text …}`. Compose with `io/with-raw-mode` / `term/with-alt-screen`.

```sema
(io/with-raw-mode
  (term/with-bracketed-paste
    (run-editor)))
```
