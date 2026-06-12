---
name: "io/read-key"
module: "terminal"
section: "Raw-Mode Input"
---

Block until a single keypress arrives, then return a map describing it. Returns `nil` on EOF (after which `io/eof?` returns `#t`).

```sema
(io/read-key)
;; => {:kind :char :char "a"}
```

The map's `:kind` field is one of:

| `:kind`   | Other keys              | Meaning                                         |
|-----------|-------------------------|-------------------------------------------------|
| `:char`   | `:char` (string)        | A printable character (UTF-8 multi-byte handled) |
| `:ctrl`   | `:char` (string)        | Ctrl + letter (e.g., Ctrl-C → `{:kind :ctrl :char "c"}`) |
| `:alt`    | `:char` (string)        | Alt/Meta + character (ESC + char sequence)      |
| `:key`    | `:name` (keyword)       | Named key — see table below                     |

Named keys (`:kind :key`) currently emitted:

`:enter` `:tab` `:backspace` `:esc` `:up` `:down` `:left` `:right` `:home` `:end` `:delete` `:page-up` `:page-down` `:f1` `:f2` `:f3` `:f4`

CSI/SS3 escape sequences (arrow keys, F1–F4, Page Up/Down, Delete) and UTF-8 continuation bytes are decoded for you with a 20 ms continuation-byte window. F5–F12 and Insert use longer escape sequences that aren't decoded yet — they fall through as raw characters.
