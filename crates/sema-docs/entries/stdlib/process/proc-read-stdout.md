---
name: "proc/read-stdout"
module: "process"
section: "Processes"
---

Drain and return everything written to the process's stdout since the last call (non-blocking; `""` if nothing new). Lets a TUI show output as it streams.
