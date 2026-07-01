---
name: "term/enter-alt-screen"
module: "terminal"
section: "Screen Control"
---

Switch to the terminal's alternate screen buffer. Use at app start so the TUI gets a clean canvas; pair with `term/leave-alt-screen` on exit to restore the user's scrollback exactly as it was. Takes no arguments.
