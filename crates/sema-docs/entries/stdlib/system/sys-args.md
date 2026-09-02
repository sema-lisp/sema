---
name: "sys/args"
module: "system"
section: "System Information"
syntax: "(sys/args)"
returns: "list"
see_also: ["env", "exit"]
---

Return the process's full argument vector as a list of strings: the
executable path first, then every argument as the shell passed it, including
the script path and any `--` separator. It is the raw `argv`, not just the
script's own arguments.

The `sema` CLI forwards arguments to a script only after `--`, so
`sema app.sema -- --verbose out.txt` gives `("sema" "app.sema" "--"
"--verbose" "out.txt")`. Drop everything up to and including `--` to get the
script's arguments; a standalone executable built with `sema build` sees its
arguments directly after the program path.

```sema
;; sema app.sema -- --verbose out.txt
(sys/args)   ; => ("sema" "app.sema" "--" "--verbose" "out.txt")

;; The script's own arguments: everything after "--".
(define (script-args)
  (let ((rest (list/drop-while (fn (a) (not (equal? a "--"))) (sys/args))))
    (if (null? rest) '() (cdr rest))))
```

Environment variables are read with `env`, not from the argument list.
