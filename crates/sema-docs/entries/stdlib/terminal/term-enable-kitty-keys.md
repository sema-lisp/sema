---
name: "term/enable-kitty-keys!"
module: "terminal"
section: "Screen Control"
---

Opt into the kitty keyboard protocol (progressive-enhancement flags: disambiguate + report-associated-text). While enabled, `io/read-key` decodes richer key events — reliable modifier reporting (an optional `:mods` list of `:shift`/`:alt`/`:ctrl`/`:super`) and unambiguous key identification — normalized to the same `{:kind :char/:ctrl/:alt/:key}` shapes as the legacy path, so existing consumers keep working. Terminals without kitty support silently ignore this and keys keep arriving via the legacy encoding. Restore with `term/disable-kitty-keys!` before leaving raw mode. Takes no arguments.
