---
name: "sys/on-signal"
module: "system"
section: "Signals"
params: [{ name: signal, type: keyword, doc: "one of :winch, :int, :term" }, { name: callback, type: function }]
returns: "nil"
see_also: ["sys/check-signals", "io/read-key"]
---

Register a callback for a signal. Multiple callbacks per signal are supported; they fire in registration order.

Supported signals:

| Keyword  | Signal     | Typical use                          |
|----------|------------|--------------------------------------|
| `:winch` | `SIGWINCH` | Terminal resize — redraw the UI      |
| `:int`   | `SIGINT`   | Ctrl-C — clean shutdown              |
| `:term`  | `SIGTERM`  | Termination request — clean shutdown |

```sema
(sys/on-signal :int (fn ()
  (println "interrupted, cleaning up")
  (exit 0)))
```
