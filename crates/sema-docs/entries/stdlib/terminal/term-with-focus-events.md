---
name: "term/with-focus-events"
module: "terminal"
section: "Screen Control"
syntax: "(term/with-focus-events body ...)"
returns: "any"
see_also: ["term/enable-focus-events", "term/disable-focus-events", "io/with-raw-mode"]
---

Guard macro: enable focus reporting, run `body`, and **always** disable it on exit — even if `body` throws. Returns `body`'s value. Inside, `io/read-key` returns focus changes as `{:kind :focus :focused #t|#f}`.

The macro returns the value of the last `body` form. If `body` throws, the terminal mode is restored first and the error is re-raised. Outside raw mode the events are still enabled, but `io/read-key` cannot read them until `io/tty-raw!` runs.

```sema
(io/with-raw-mode
  (term/with-focus-events
    (let ((k (io/read-key)))
      (when (= (:kind k) :focus) (println (:focused k))))))
```
