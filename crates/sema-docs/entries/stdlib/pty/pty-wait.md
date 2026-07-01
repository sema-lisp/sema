---
name: "pty/wait"
module: "pty"
section: "Pseudo-Terminals"
---

Block until the child exits and return its exit code. All output is buffered first, so a following `pty/read` returns the tail.
