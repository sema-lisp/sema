---
name: "sys/config-dir"
module: "system"
section: "System Information"
syntax: "(sys/config-dir)"
returns: "string"
---

Return the platform-appropriate base directory for user configuration, so apps can locate their config without branching on the operating system.

| Platform | Location |
|----------|----------|
| Linux    | `$XDG_CONFIG_HOME` or `~/.config` |
| macOS    | `~/Library/Application Support` |
| Windows  | `%APPDATA%` |

```sema
(path/join (sys/config-dir) "sema" "sema-code" "config.json")
```
