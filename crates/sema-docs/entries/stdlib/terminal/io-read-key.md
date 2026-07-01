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
| `:mouse`  | `:action` `:x` `:y` `:button` `:mods` | A mouse event (only after `term/enable-mouse`)  |

Named keys (`:kind :key`) currently emitted:

`:enter` `:tab` `:backspace` `:esc` `:up` `:down` `:left` `:right` `:home` `:end` `:delete` `:page-up` `:page-down` `:f1` `:f2` `:f3` `:f4`

CSI/SS3 escape sequences (arrow keys, F1–F4, Page Up/Down, Delete) and UTF-8 continuation bytes are decoded for you. F5–F12 and Insert use longer escape sequences that aren't decoded yet — they fall through as raw characters.

**Mouse** (after `term/enable-mouse`): SGR mouse reports decode to
`{:kind :mouse :action A :x col :y row :button N :mods (…)}`, where `A` is one of
`:press` `:release` `:move` `:wheel-up` `:wheel-down` `:wheel-left` `:wheel-right`,
coordinates are 1-based, and `:mods` (omitted when empty) lists `:shift`/`:alt`/`:ctrl`.

**Kitty keyboard** (after `term/enable-kitty-keys!`): richer key events decode to
the *same* `:char`/`:ctrl`/`:alt`/`:key` shapes above — so existing code is
unaffected — plus an optional `:mods` list (e.g. Shift+A →
`{:kind :char :char "A" :mods (:shift)}`). Terminals without kitty support keep
the legacy encoding. Both mouse and kitty decoding are opt-in; plain keys are
byte-identical whether or not they're enabled.
