---
name: "shell/quote"
module: "system"
section: "Shell & Process Control"
syntax: "(shell/quote s)"
returns: "string"
---

POSIX single-quote a string so it survives a POSIX shell (e.g. `sh -c`) as one literal word —
no metacharacter is special inside single quotes, so this defuses command injection. Wraps the
value in single quotes and rewrites each embedded `'` as `\'\''`; the empty string becomes `''`.
Note: the single-string form of `shell` uses `sh -c` on Unix but `cmd /C` on Windows.

```sema
(shell/quote "a b")            ; => "'a b'"
(shell/quote "a'b")            ; => "'a'\\''b'"
(shell/quote "")               ; => "''"

(shell (str "echo " (shell/quote "hi; rm -rf /")))
; => {:exit-code 0 :stdout "hi; rm -rf /\n" ...}  ; the payload is echoed, not run
```
