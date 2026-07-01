---
name: "term/disable-kitty-keys!"
module: "terminal"
section: "Screen Control"
---

Pop the kitty keyboard protocol flags pushed by `term/enable-kitty-keys!`, restoring the terminal's previous keyboard mode. Call before `io/tty-restore!` on exit. Takes no arguments.
