---
name: "term/enable-kitty-keys!"
module: "terminal"
section: "Screen Control"
params: [{ name: flags, type: int, doc: "optional; defaults to 17" }]
returns: "nil"
see_also: ["term/disable-kitty-keys!", "term/with-kitty-keys", "term/supports-kitty-keys?", "term/query-kitty-keys"]
---

Opt into the kitty keyboard protocol. While enabled, `io/read-key` decodes richer key events — reliable modifier reporting (an optional `:mods` list of `:shift`/`:alt`/`:ctrl`/`:super`/`:hyper`/`:meta`/`:caps-lock`/`:num-lock`) and unambiguous key identification — normalized to the same `{:kind :char/:ctrl/:alt/:key}` shapes as the legacy path, so existing consumers keep working. Terminals without kitty support silently ignore this and keys keep arriving via the legacy encoding. Restore with `term/disable-kitty-keys!` before leaving raw mode (or use the `term/with-kitty-keys` guard).

Takes an optional flags bitmask (default `17` = disambiguate `1` + report-associated-text `16`) — the conservative default omits event types, so no repeat/release events double-fire. Pass a larger mask to request more, e.g. add `2` for event types (surfaced as `:event :press|:repeat|:release`) or `4` for alternate keys (`:shifted-key`/`:base-key`):

```sema
(term/enable-kitty-keys!)      ; default flags 17
(term/enable-kitty-keys! 19)   ; 17 + event types
```

Use `term/supports-kitty-keys?` to detect support first, and `term/query-kitty-keys` to read the terminal's active flags.
