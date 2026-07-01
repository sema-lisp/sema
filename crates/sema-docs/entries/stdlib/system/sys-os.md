---
name: "sys/os"
module: "system"
section: "System Information"
---

Return the raw operating system name from the Rust target (`std::env::consts::OS`).
This is an **open set** — besides `"macos"`, `"linux"`, and `"windows"` it can also
report `"ios"`, `"android"`, `"freebsd"`, `"dragonfly"`, and others.

```sema
(sys/os)   ; => "macos"
```

Use `sys/platform` instead when you want a **closed, normalized set**
(`"macos"` / `"linux"` / `"windows"` / `"unknown"`) that is safe to `match`/`cond`
on exhaustively.
