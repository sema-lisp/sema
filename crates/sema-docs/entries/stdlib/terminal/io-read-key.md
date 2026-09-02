---
name: "io/read-key"
module: "terminal"
section: "Raw-Mode Input"
syntax: "(io/read-key)"
returns: "map or nil"
see_also: ["io/read-key-timeout", "io/tty-raw!", "io/with-raw-mode", "term/enable-mouse"]
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
| `:alt`    | `:char` (string)        | Alt/Meta + character (ESC + char; UTF-8 aware)  |
| `:key`    | `:name` (keyword), optional `:mods` | Named key — see table below         |
| `:mouse`  | `:action` `:x` `:y` `:button` `:mods` | A mouse event (after `term/enable-mouse`) |
| `:paste`  | `:text` (string)        | A bracketed paste (after `term/enable-bracketed-paste`) — pasted text delivered whole, so its newlines/control bytes aren't read as keystrokes |
| `:focus`  | `:focused` (bool)       | Window focus gained/lost (after `term/enable-focus-events`) |
| `:cpr`    | `:row` `:col`           | Cursor-position report — reply to `term/query-cursor-position` |
| `:device-attributes` | `:device` (`:primary`/`:secondary`) `:params` | Reply to `term/query-primary-da` / `term/query-secondary-da` |
| `:kitty-flags` | `:flags` (int)     | Reply to `term/query-kitty-keys` |

Named keys (`:kind :key`):

`:enter` `:tab` `:backspace` `:esc` `:up` `:down` `:left` `:right` `:home` `:end` `:insert` `:delete` `:page-up` `:page-down` `:shift-tab` `:f1`–`:f12`

CSI/SS3 escape sequences (arrows, F1–F12, Insert, Home/End, Page Up/Down, Delete) and UTF-8 continuation bytes are decoded for you. **Modifier-carrying keys** include an optional `:mods` list — e.g. Ctrl+Right → `{:kind :key :name :right :mods (:ctrl)}`, Shift+F5 → `{:kind :key :name :f5 :mods (:shift)}` (xterm modifier form and `modifyOtherKeys` are both decoded).

**Mouse** (after `term/enable-mouse`): SGR mouse reports decode to
`{:kind :mouse :action A :x col :y row :button N :mods (…)}`, where `A` is one of
`:press` `:release` `:move` `:wheel-up` `:wheel-down` `:wheel-left` `:wheel-right`,
coordinates are 1-based, and `:mods` (omitted when empty) lists `:shift`/`:alt`/`:ctrl`.

**Kitty keyboard** (after `term/enable-kitty-keys!`): richer key events decode to
the *same* `:char`/`:ctrl`/`:alt`/`:key` shapes above — so existing code is
unaffected — plus an optional `:mods` list (full set: `:shift` `:alt` `:ctrl`
`:super` `:hyper` `:meta` `:caps-lock` `:num-lock`). When event types are enabled
(flag `2`), events carry `:event :press|:repeat|:release`; with alternate keys
(flag `4`) they carry `:shifted-key`/`:base-key`. Terminals without kitty support
keep the legacy encoding. All of mouse, kitty, paste, and focus decoding are
opt-in; plain keys are byte-identical whether or not they're enabled.
