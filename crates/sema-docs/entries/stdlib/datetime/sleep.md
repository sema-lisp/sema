---
name: "sleep"
module: "datetime"
section: "Delay"
params: [{ name: milliseconds, type: int }]
returns: "nil"
---

Pause execution for a given number of milliseconds. Returns `nil`.

```sema
(sleep milliseconds) ; => nil
```

```sema
(sleep 1000)  ; sleep for 1 second
(sleep 500)   ; sleep for 500ms
(sleep 0)     ; yield (no-op pause)
```

Note that `sleep` takes **milliseconds** (not seconds), unlike the `time/` functions which work in seconds.
