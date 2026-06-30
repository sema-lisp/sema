---
name: "proc/wait"
module: "process"
section: "Processes"
---

Block until the process exits and return its exit code (`-1` if killed by a signal). Reader threads finish flushing first, so a subsequent `proc/read-stdout` returns the tail.
