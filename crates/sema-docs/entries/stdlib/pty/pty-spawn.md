---
name: "pty/spawn"
module: "pty"
section: "Pseudo-Terminals"
---

Spawn a command under a pseudo-terminal and return an integer handle. `(pty/spawn ["bash"] {:rows 40 :cols 120 :cwd "path" :env {...}})`. The child sees a real TTY (isatty is true), so REPLs, editors, and color-aware tools behave normally. Output (stdout+stderr merged) streams into a buffer you drain with `pty/read`.
