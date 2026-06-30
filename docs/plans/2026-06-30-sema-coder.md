# Sema Coder — triage of issue #53 (host/app primitives)

Issue #53 brainstorms the primitives that would make a "coding agent written
almost entirely in Sema" feasible. It is a long wishlist (13 categories). This
note records the triage and what shipped on
`claude/sema-coder-agent-primitives-6gxxo4`.

## Decision

Scope to **the TUI + app shell**, per the issue's own "uncomfortable truth":
the stdlib is already strong for scripting; what's missing is the handful of
*host* primitives that make Sema feel like an application runtime — and a
canonical app ("Sema Coder") that proves the loop end-to-end and is easy to
extend with new slash commands.

We deliberately did **not** clone every wishlist item. MCP-related work waits on
a separate branch (per the issue and the owner's direction).

## Shipped — Rust primitives

Small, safe, high-leverage. All on stable APIs, no new dependencies.

**Terminal screen control** (`sema-stdlib/src/terminal.rs`) — emit ANSI/VT
sequences so TUIs stop hand-writing escape codes:

```
term/enter-alt-screen  term/leave-alt-screen
term/clear  term/clear-line  term/clear-below
term/move-to  term/write-at  term/cursor-home
term/hide-cursor  term/show-cursor  term/save-cursor  term/restore-cursor
term/enable-mouse  term/disable-mouse
term/set-title  term/bell  term/flush
```

**Path safety** (`sema-stdlib/src/io.rs`):

```
path/canonicalize   ;; resolve symlinks + `.`/`..` (errors if missing)
path/relative-to    ;; pure path math: express PATH relative to BASE
path/within?        ;; containment check — the cornerstone of agent sandboxing
```

**Config location** (`sema-stdlib/src/system.rs`):

```
sys/config-dir      ;; platform config base (XDG / Application Support / APPDATA)
```

Tests: `crates/sema/tests/eval_test.rs` (path math, containment, config-dir,
terminal no-ops).

## Shipped — the app

`examples/sema-coder/` — a coding agent in Sema, reusing `defagent`/`deftool`/
`agent/run`, `file/*`, `shell`, `json/*`, and the new `term/*` + `path/within?`.

- Single JSON config at `<config-dir>/sema/sema-code/config.json`.
- **Extensible slash commands** two ways: a Sema registry (`register-command!`,
  one call) and declarative config entries (`commands` map → shell templates with
  `$ARGS`).
- On-brand styling (sema gold `#c8a855`) and a compact wordmark banner.

## Wave 2 — shipped

The deferred primitives are now implemented as native stdlib modules (the
independent ones were authored in parallel by subagents, then integrated and
verified together). Native-only modules are gated `cfg(not(wasm32))`; pure ones
(`diff`, `secret`, `reflect`) compile for all targets.

- **Streaming processes** (`proc.rs`) — `proc/spawn` `read-stdout` `read-stderr`
  `write-stdin` `close-stdin` `wait` `exit-code` `running?` `kill` `close`.
  Background reader threads drain pipes into buffers you poll, so test output
  streams live. (PTY is still future — not MVP.)
- **Event model** (`event.rs`) — `event/select` (poll-based selector over
  `:key` / `:proc` / `:timer` sources) + `time/tick`.
- **Diff/patch** (`diff.rs`, `similar` crate) — `diff/unified` `parse` `apply`
  `hunks` `stat` + `patch/apply-file`.
- **Git** (`git.rs`, shells out) — `git/root` `current-branch` `status`
  `changed-files` `diff` `diff-files` `recent-files` `ignore-matches?`.
- **fs watching** (`fs_watch.rs`, `notify`) — `fs/watch` `watch-events` `unwatch`.
- **Sema reflection + diagnostics** (`reflect.rs`) — `read/string` `read/all`
  `format/form`, and `sema/check-string` / `sema/check-file` returning
  `{:ok :diagnostics}` as data for agent repair loops.
- **Secrets/PII** (`secret.rs`) — `secret/detect` `secret/redact` `pii/detect`
  `redact/spans` `hash/digest` (regex + entropy detectors).
- **Archives** (`archive.rs`) — `gzip/compress` `gzip/decompress` `zip/create`
  `zip/extract` `zip/list` `tar/create` `tar/extract` (zip-slip guarded).
- **Markup** (`markup.rs`, `pulldown-cmark` + `scraper`) — `markdown/to-html`
  `markdown/headings` `markdown/frontmatter` `html/parse` `html/select`
  `html/select-text` `html/text`.

Each module ships unit tests; all 53 new builtins have doc entries (coverage
gate green).

## Still deferred

- **PTY** (`pty/spawn`/`resize`/`read`/`write`) — pseudo-terminal allocation;
  `proc/*` covers streaming subprocesses, which was the MVP need.
- **buffer/editor layer** and **test-harness DSL** (`deftest`/`expect`) — these
  are best written in Sema (prelude/package level), not as Rust primitives.
- **`ast/spans`** — requires the reader to carry span info on the Value AST;
  `ast/symbols`/`find`/`rewrite` are expressible in Sema over quoted forms.
