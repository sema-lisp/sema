---
name: "sys/on-signal"
module: "system"
section: "Signals"
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
