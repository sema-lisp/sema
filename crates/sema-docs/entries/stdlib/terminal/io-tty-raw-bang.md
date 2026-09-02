---
name: "io/tty-raw!"
module: "terminal"
section: "Raw-Mode Input"
syntax: "(io/tty-raw!)"
returns: "int or nil"
see_also: ["io/tty-restore!", "io/with-raw-mode", "io/read-key"]
---

Put stdin into raw mode. Returns an **integer restore-token** on success, or `nil` if stdin is not a TTY (e.g., when input is piped from a file). Always pair with `io/tty-restore!` so the user's shell isn't left in raw mode if your program crashes.

```sema
(define tok (io/tty-raw!))
(when tok
  ;; ... read keys, draw UI ...
  (io/tty-restore! tok))
```
