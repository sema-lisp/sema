---
name: "term/with-mouse"
module: "terminal"
section: "Screen Control"
syntax: "(term/with-mouse body ...)"
returns: "any"
see_also: ["term/enable-mouse", "term/disable-mouse", "io/with-raw-mode", "term/with-alt-screen"]
---

Guard macro: enable mouse reporting (`term/enable-mouse`), run `body`, and
**always** disable it on exit — even if `body` throws (the error is re-raised
after disabling). Returns `body`'s value. Without the guard, a crash leaves mouse
reporting on and escape reports spew into the shell as garbage. `io/read-key`
decodes reports as `{:kind :mouse …}` while enabled.

The macro returns the value of the last `body` form. If `body` throws, the terminal mode is restored first and the error is re-raised. Outside raw mode the events are still enabled, but `io/read-key` cannot read them until `io/tty-raw!` runs.

```sema
(io/with-raw-mode
  (term/with-mouse
    (let ((k (io/read-key)))
      (when (= (:kind k) :mouse) (println (:x k) (:y k))))))
```
