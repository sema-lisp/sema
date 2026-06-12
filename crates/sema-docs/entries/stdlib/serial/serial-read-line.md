---
name: "serial/read-line"
module: "serial"
section: "I/O"
---

```sema
(serial/read-line handle) → string
```

Read until `\n`, then trim trailing `\r` / `\n` and return the line. Blocks until either a newline arrives or the port's read timeout elapses (configured at `serial/open` time) — on timeout, raises an error.

```sema
(serial/read-line pico)   ; => "ready"
```
