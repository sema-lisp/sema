---
name: "term/flush"
module: "terminal"
section: "Screen Control"
---

Flush buffered stdout. The other `term/*` control functions self-flush; use this when you batch styled `io/print` writes and want to present a frame all at once. Takes no arguments.
