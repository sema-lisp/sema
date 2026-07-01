---
name: "sys/platform"
module: "system"
section: "System Information"
---

Return a normalized platform name, always one of the **closed set**
`"macos"`, `"linux"`, `"windows"`, or `"unknown"`. Anything the build doesn't
recognize collapses to `"unknown"`, so this is safe to branch on exhaustively.

```sema
(sys/platform)   ; => "macos" / "linux" / "windows"
```

For the raw, open-ended OS name (e.g. `"ios"`, `"android"`, `"freebsd"`), use
`sys/os`.
