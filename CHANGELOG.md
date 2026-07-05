# Changelog

## Unreleased

### Changed

- **Package registry (`pkg/`) now runs on SQLite, PostgreSQL, and MySQL from one
  binary**, with a clean Data Access Layer (per
  `docs/plans/2026-07-05-pkg-dal-multi-engine.md`). The engine is inferred from
  the `DATABASE_URL` scheme; `db::connect()` applies SQLite-only tuning where
  relevant and runs SeaORM programmatic migrations (`src/migration/`) that emit
  correct DDL per engine, replacing the SQLite-dialect `migrations/*.sql`. All
  database access moved out of handlers into per-aggregate modules under
  `src/dal/` (`packages`, `versions`, `owners`, `deps`, `users`, `sessions`,
  `tokens`, `reports`, `audit_log`, `oauth`, `downloads`, `admin`, `sync_log`,
  `time`); handlers are now parse/authorize → call DAL → shape response and
  contain no SQL. Portability is by construction: timestamps are generated in
  Rust (no `datetime('now')`/`CURRENT_TIMESTAMP`), upserts use SeaORM
  `on_conflict`, and any raw SQL is standard and parameterized. `make
  test-all-drivers` runs the suite against all three engines. Removed the unused
  `SESSION_SECRET` config. HTTP behavior is unchanged.

### Security

- **Package registry (`pkg/`) — stored-XSS-to-admin-takeover and secret-default
  fixes (from an adversarial review).** Published package names are now
  validated against a strict allowlist (`[A-Za-z0-9._-]`, alnum-bounded, no
  `..`) on both CLI publish and GitHub link; previously a name taken verbatim
  from the URL was interpolated into the package page's Alpine `x-init`/`@click`
  JavaScript, so a crafted name (e.g. `');fetch('/api/v1/admin/users/…/role',
  {method:'PUT',body:'{"is_admin":true}'})//`) could run in an admin's browser
  and self-promote to admin. `repository_url` is now required to be `http(s)`
  (blocks a `javascript:` link on the package page). The server refuses to boot
  when GitHub OAuth is enabled but `OAUTH_TOKEN_KEY` is left at the insecure
  compiled-in default (stored GitHub tokens are AES-encrypted with it). The
  webhook handler returns a uniform `403` for an unknown repo so the status code
  can't be used to enumerate linked repositories. Removed the unused
  `SESSION_SECRET` config (sessions use opaque random DB-backed ids; the key was
  never referenced).
- **Package registry (`pkg/`) auth hardening ahead of live deploy.** Logout now
  deletes the session row server-side, so a captured session cookie can no
  longer be replayed after the user logs out (previously the cookie stayed
  valid in the DB for its full 7-day lifetime). The GitHub OAuth `return_to`
  parameter is restricted to same-site paths (`auth::sanitize_return_to`),
  closing an open redirect where `/auth/github?return_to=https://evil.com`
  would bounce a logged-in user off-site. The webhook handler now refuses a
  push when the linked package has an empty/missing webhook secret, instead of
  validating the signature against an attacker-computable empty-key HMAC.
  Session cookies gain the `Secure` attribute automatically when `BASE_URL` is
  `https://` (kept off for local `http://` dev). Added regression tests for
  each, alongside the existing admin-authorization coverage.

### Fixed

- **Package registry (`pkg/`) pre-deploy hardening** (per
  `docs/plans/2026-06-09-pkg-registry-predeploy-hardening.md`): publish is now
  atomic — package, owner, version, and dependency rows are written in a single
  transaction, and dependency-insert errors propagate instead of being silently
  swallowed (previously a version row could commit with missing dependency
  rows). Uploads are validated up front: gzip magic bytes required, dependency
  count capped (`MAX_DEPENDENCIES`, default 64), `version_req` strings parsed
  with `semver`, and malformed metadata JSON rejected with 400 instead of
  silently dropping dependencies/description. `blob::store` returns IO errors
  as 500s instead of panicking the handler. Also fixed axum's 2 MB default
  body cap silently overriding `MAX_TARBALL_BYTES` on the publish route, and
  consolidated API error responses into a shared `ApiError` type.

### Added

- **WebSocket client (`ws/*`).** Connect to `ws://`/`wss://` servers with
  `ws/connect` (options map: `:headers`, `:subprotocols`, `:timeout`, `:retries`,
  `:retry-backoff-ms` with exponential backoff), then `ws/send`, `ws/recv`,
  `ws/recv-timeout`, `ws/ping`, `ws/close`, and `ws/connected?`. A connection is a
  closeable stream, so the new `with-open` macro (a RAII alias of `with-stream`)
  closes it on both the normal and error paths. `ws/send` accepts a string (text),
  a bytevector (binary), a map (JSON text), or explicit framing (`{:text …}`,
  `{:binary …}`, `{:json …}`); `ws/recv` returns a tagged map (`{:text …}`,
  `{:binary …}`, `{:close …}`, or `nil` once drained) for `match`, and
  `ws/recv-timeout` returns `:timeout` distinct from `nil`. The `ws/listen` macro
  drives an evented receive loop dispatching to
  `:on-open`/`:on-message`/`:on-close`/`:on-error` and returns a promise to await.
  Top-level calls block; inside an `async/spawn` task they yield cooperatively,
  mirroring the HTTP client's offload model. Gated on the `network` capability.
  Also runs in the **browser** (Sema Web / WASM) over the native `WebSocket`:
  `ws/connect`, `ws/send`, `ws/close`, `ws/connected?`, and `ws/listen` all work
  there; the pull-based `ws/recv`/`ws/recv-timeout` stay native-only (the browser
  main thread can't block — receive via the evented `ws/listen`), and only
  `:subprotocols` of the connect options apply in the browser. **Binary frames
  round-trip end-to-end** — client, browser, and the server-side `:ws` handler
  all carry binary: server `:send` accepts a bytevector (binary frame) and
  `:recv` surfaces a binary frame as a bytevector (text frames stay strings),
  and browser `ws/send`/`ws/listen` marshal bytevectors as `Uint8Array` across
  the WASM boundary. The server side (`:ws` routes / `http/websocket`) already
  shipped. (#49)
- **Self-tail-call optimization: named-let loops no longer birth a self-reference
  cycle (issue #62).** A self-recursive named-let / `letrec` loop whose name is
  referenced only in tail-call position no longer captures itself as an upvalue.
  The resolver detects the pattern and elides the self upvalue
  (`VarResolution::SelfFn`); the compiler emits a new `SelfTailCall` opcode that
  reuses the running frame's own closure instead of `LoadUpvalue`+`TailCall`. This
  removes the CORE-2 self-reference cycle (ADR #66) at its hottest source — every
  loop *entry* previously birthed a 3-node cycle for the collector to trace and
  sever — and lets pure counter loops reach zero upvalues (skipping cycle-candidate
  registration entirely). Measured ~22% faster on `mandelbrot.sema` (named-let per
  pixel) with identical output. The optimization is conservative: it does not fire when
  the loop name escapes (stored, passed, returned, `set!`, captured by an inner
  lambda, or used in non-tail position) — those keep the real self-capture.
  Verified by resolver/compiler unit tests, end-to-end eval oracles (including the
  upvalue-index remap when a loop also captures outer variables), a `.semac`
  round-trip + verifier test, and `gc_stress_test` no-cycle assertions.
- **Sema Web — run Sema apps in the browser.** The new `@sema-lang/sema-web`
  package embeds the WASM VM in the browser with reactive state
  (`state`/`computed`/`watch`), SIP markup (hiccup-style vectors), a component
  system (`defcomponent`/`mount!`), and `dom/*`, `store/*`, `router/*`, `css/*`,
  and browser `llm/*` namespaces. `@sema-lang/llm-proxy` ships drop-in Vercel /
  Netlify / Cloudflare / Node adapters so browser `llm/*` calls reach real
  providers with server-side keys. See the
  [Sema Web guide](https://sema-lang.com/docs/web/).
- **`sema web` — zero-config dev server.** `sema web app.sema` serves an app in
  the browser with no bundler and no `npm install`: it embeds the browser runtime
  (WASM VM + JS bundle) in the binary, serves your app, hot-reloads on file
  change, and proxies `llm/*` to real providers using your environment keys.
  Multi-file apps (that `import` other modules) are compiled to a `.vfs` on the
  fly and resolve automatically, and a browser error overlay surfaces Sema errors
  on the page. Options: `--port`, `--host`, `--no-open`, `--no-llm`. See the
  [Dev Server guide](https://sema-lang.com/docs/web/dev-server).
- **Automatic port fallback for `http/serve`.** Pass `{:port-fallback true}` and
  a taken port advances to the next free one instead of failing;
  `{:on-listen (fn (info) …)}` reports the bound `{:host :port :url}` (handy for
  printing a URL or opening a browser). Off by default (backward-compatible); the
  notebook server opts in. See the
  [Web Server docs](https://sema-lang.com/docs/stdlib/web-server).
- **Transitive dependency resolution for `sema pkg`.** `sema pkg install` (and
  `add`/`update`/`remove`) now walks each dependency's own `[deps]` and installs
  the whole graph, instead of only the project's top-level manifest — no more
  hand-flattening a dependency tree into your own `sema.toml`. Diamond conflicts
  resolve deterministically: direct deps always win over transitive requests,
  semver-compatible transitive versions pick the higher one, and incompatible
  majors / conflicting git refs hard-error asking for an explicit override.
  `sema.lock` records a per-entry `direct` flag (additive; defaults to `true`
  when absent, so existing lock files are unaffected) so `--locked` and
  `sema pkg list` can distinguish direct from transitive packages.
- **CORE-2 fixed: cycle-collecting garbage collector.** Sema now reclaims reference
  cycles — a synchronous Bacon–Rajan cycle collector (ADR #66; design + measurements
  in `docs/plans/2026-07-02-core2-gc.md`) runs over the existing `Rc` heap with a
  creation-time candidate registry, reclaiming garbage cycles by severing the one
  mutable cell every Sema cycle must pass through (env bindings, upvalue cell, thunk
  `forced`, promise state, channel buffer, multimethod table) and letting ordinary
  `Rc` drops cascade. What users get: **long-lived sessions stop leaking** — REPL,
  notebook server, HTTP/MCP servers, and long-running agents that define recursive
  local helpers every turn (previously 260 B leaked per recursive closure, forever)
  now stay memory-bounded, and **interpreter teardown frees everything** (previously
  every `Interpreter` drop leaked its entire global env, ~168 KB, even with zero
  user code — fatal for embedders and notebook kernel resets). Measured by the
  counting-allocator oracles in `crates/sema/tests/leak_test.rs` (all seven green):
  recursive-closure churn 260 B/iter → ~0 (bounded); teardown ~168 KB/drop → 0 B/drop
  (no defines, macro-injected consts, module imports) and bounded with user defines;
  1M-iteration churn RSS 303.7 MB → 16.0 MB. Collection runs at safe points (closure/
  data-birth registry threshold, top-level eval return, notebook cell + kernel reset,
  agent-turn boundary, scheduler idle, `Interpreter::drop`) plus on demand via the new
  **`gc/collect` and `gc/stats` builtins** and the REPL `,gc` command. Perf gate
  (interleaved hyperfine A/B vs the pre-collector baseline): closure bookkeeping
  ≤1.6% on storm/upvalue-counter/fold; numeric within noise (tak's +0.9% shows
  all-zero `gc/stats` — layout noise); reclamation costs ~326 ns per collected cycle
  at 1.73× the *leaking* baseline's wall time on the churn canary; mandelbrot pays
  +12% because its named-`let` loops birth a real cycle per entry — the baseline
  leaks on it, and issue #62 (self-tail-call optimization) is the planned
  elimination. Load-bearing invariant established in M1 (AGENTS.md, I2): a
  `NativeFn`'s boxed closure must never strongly capture anything that can
  transitively hold a `Value`/`Env` — traceable state belongs in `NativeFn.payload`;
  the ~11 `__vm-*`/tool/agent delegates now capture their home env `Weak`. No object
  headers, no color bits, no change to `Value`'s NaN-boxing, `Value::drop`, or the
  strong-reference graph user code sees.

- **`otel/configure` — turn on tracing from Sema code.** Telemetry no longer
  needs environment variables: `(otel/configure {:endpoint "..." :key "sk_..."})`
  points Sema at an OTLP backend (or a JSONL file via `:file`) from inside a
  script. `:key` becomes an `Authorization: Bearer` header; `:headers` takes a
  map or a pre-formatted string; `:service-name`, `:environment`, `:release`, and
  `:capture-content` mirror their env vars. Installs one provider per process and
  returns `#t` when it turned tracing on (env config, if present, still wins).
  See the [Observability guide](https://sema-lang.com/docs/llm/observability#configuring-from-sema-code).
- **MCP client.** Sema can now act as an MCP *client*, not just a server, over
  every standard transport. `mcp/connect` picks the transport from its config:
  `:command` spawns a **stdio** server (gated on the `process` capability;
  credentials via the `:env` map), `:url` connects to a remote server over
  **Streamable HTTP** (gated on `network`; MCP spec `2025-11-25`, with
  `Mcp-Session-Id` / `MCP-Protocol-Version` handling and JSON-or-SSE responses),
  and it auto-falls-back to the deprecated 2024-11-05 HTTP+SSE transport when a
  server only speaks that. `mcp/tools` lists tools, `mcp/call` invokes one,
  `mcp/close` disconnects, and `mcp/tools->sema` converts a server's tools into
  the exact value shape `deftool` produces so `defagent` consumes external MCP
  tools with no agent-loop changes (`isError` surfaces as an error).
- **MCP client OAuth 2.1 login.** Remote servers that require authorization are
  handled natively per the MCP authorization spec: on a `401`, Sema discovers the
  authorization server (RFC 9728 protected-resource metadata → RFC 8414/OIDC
  metadata), registers a client (RFC 7591 dynamic registration, a pre-registered
  `:auth {:client-id …}`, or a cached one), and runs the Authorization-Code +
  PKCE-S256 flow over an RFC 8252 loopback redirect — opening the system browser,
  binding `resource=` (RFC 8707), then exchanging the code for tokens. Tokens are
  cached in the OS keychain (with a `0600`-file fallback), so later connects are
  silent; expired tokens are refreshed automatically. A headless RFC 8628
  device-authorization flow and a bring-your-own-token option (`:headers`) are
  also supported. `sema mcp login <url>` (with `--device` / `--client-id`) and
  `sema mcp logout <url>` manage credentials from the CLI. A mid-session `401`
  (expired token) or `403 insufficient_scope` re-authorizes and retries the call
  transparently — refreshing, or stepping up to the union of scopes — on both the
  Streamable-HTTP and legacy HTTP+SSE transports. Set `SEMA_MCP_TOKEN_STORE=file`
  to force the `0600`-file store instead of the OS keychain (handy on headless
  boxes or to avoid repeated keychain prompts while developing).
- **MCP tool-call cassettes.** MCP `tools/call` results record and replay through
  the same cassette tape as LLM calls (`llm/cassette-load`/`llm/cassette-save`,
  or the `SEMA_LLM_CASSETTE` env var), keyed by a hash of the server identity,
  tool, and arguments — so an agent-over-MCP flow can be captured once and
  replayed offline/deterministically in CI, with no network or live server.

- **Agent & TUI host primitives** (issue #53) — the building blocks for
  self-hosted terminal apps written in Sema (see the
  [Sema Coder](https://github.com/HelgeSverre/sema/tree/main/examples/sema-coder)
  reference app; the primitives are documented per module under the
  [standard library reference](https://sema-lang.com/docs/stdlib/)):
  - **Terminal screen control** — `term/enter-alt-screen`, `term/leave-alt-screen`,
    `term/clear`, `term/clear-line`, `term/clear-below`, `term/move-to`,
    `term/write-at`, `term/cursor-home`, `term/hide-cursor`, `term/show-cursor`,
    `term/save-cursor`, `term/restore-cursor`, `term/enable-mouse`,
    `term/disable-mouse`, `term/set-title`, `term/bell`, `term/flush`.
  - **Streaming subprocesses** — `proc/spawn`, `proc/read-stdout`,
    `proc/read-stderr`, `proc/write-stdin`, `proc/close-stdin`, `proc/wait`,
    `proc/exit-code`, `proc/running?`, `proc/kill`, `proc/close`.
  - **Pseudo-terminals** — `pty/spawn`, `pty/read`, `pty/write`, `pty/resize`,
    `pty/wait`, `pty/exit-code`, `pty/running?`, `pty/kill`, `pty/close`.
  - **Event loop** — `event/select` (over `:key`/`:proc`/`:timer` sources) and
    `time/tick`.
  - **File watching** — `fs/watch`, `fs/watch-events`, `fs/unwatch`.
  - **Diff & patch** — `diff/unified`, `diff/parse`, `diff/apply`, `diff/hunks`,
    `diff/stat`, `patch/apply-file`.
  - **Read-only git** — `git/root`, `git/current-branch`, `git/status`,
    `git/changed-files`, `git/diff`, `git/diff-files`, `git/recent-files`,
    `git/ignore-matches?`.
  - **Sema reflection & diagnostics** — `read/string`, `read/all`,
    `format/form`, `sema/check-string`, `sema/check-file` (diagnostics as data).
  - **Secrets & redaction** — `secret/detect`, `secret/redact`, `pii/detect`,
    `redact/spans`, `hash/digest`.
  - **Archives** — `gzip/compress`, `gzip/decompress`, `zip/create`,
    `zip/extract`, `zip/list`, `tar/create`, `tar/extract`.
  - **Markdown & HTML** — `markdown/to-html`, `markdown/headings`,
    `markdown/frontmatter`, `html/parse`, `html/select`, `html/select-text`,
    `html/text`.
  - **Path safety & config** — `path/canonicalize`, `path/relative-to`,
    `path/within?`, `sys/config-dir`.
  - **Display-aware text** — `string/width` (terminal display columns; wide-char
    and ANSI aware) and `string/word-wrap` (width-aware word wrapping).
  - **Rich terminal input** — `io/read-key` now decodes the kitty keyboard
    protocol (modifier reporting as an optional `:mods` list) and SGR mouse
    reports (`{:kind :mouse …}`), both backward compatible; opt in with
    `term/enable-kitty-keys!` / `term/disable-kitty-keys!` (mouse via the
    existing `term/enable-mouse`, which now also reports drag).
  - **Terminal setup guards** — `term/with-alt-screen`, `io/with-raw-mode`, and
    `term/with-mouse` run a body and *always* restore the terminal on exit (even
    if the body throws), so a crash can't leave the shell in raw mode / the alt
    buffer / with mouse reporting on. Compose them outermost-restores-last.
- **Streaming agent turns** — `agent/run` accepts an `:on-text` callback that
  streams the assistant reply token-by-token (in addition to `:on-tool-call`),
  so front-ends can render a reply as it arrives.
- **`string->bytevector` / `bytevector->string`** — intuitive aliases for
  `string->utf8` / `utf8->string` (a Sema string encodes to its UTF-8 bytes).
- **Sema Coder** (`examples/sema-coder/`) — a terminal coding agent written in
  Sema: a full-screen, frame-diffed TUI with a fuzzy `/` command palette, live
  streaming, mouse-wheel scrolling, resize handling, and an extensible
  slash-command registry + single-file JSON config (falls back to a plain
  line-based REPL when stdout isn't a TTY).
- **Conversation inspection, surgery, search & prompt algebra** (issue #12) —
  15 builtins for working with conversations as data: `conversation/length`,
  `conversation/turns`, `conversation/models-used`, and `conversation/stats`
  inspect a conversation; `conversation/remove`, `conversation/insert`,
  `conversation/replace`, and `conversation/map-role` edit it non-destructively;
  `conversation/search` / `conversation/find` locate messages; and the prompt
  algebra `prompt/diff`, `prompt/union`, `prompt/intersection`,
  `prompt/difference` compare message sets by role+content. `conversation/cost`
  reports the billed total.

### Fixed

- **`llm/stream` over `http/stream` streams progressively without panicking.**
  The SSE channel was bounded and used a blocking send, which panicked ("cannot
  block the current thread from within a runtime") when a handler fed tokens from
  inside a provider's async runtime. It is now an unbounded, non-blocking channel,
  so LLM tokens flow to the client as they arrive. WebSocket/file/raw responses
  are unchanged.
- **Robust module/import resolution in embedded and `.vfs` contexts.** Imports
  written `./x`, `../x`, subdirectory, and nested-relative paths now resolve
  correctly from a `sema build` standalone binary or a browser `.vfs` archive —
  previously only an exact plain relative key matched, so multi-file apps broke
  once bundled. Every spelling of a module normalizes to one key (so diamonds
  dedup to a single evaluation), and a circular `(load ...)` now errors gracefully
  instead of overflowing the stack.
- **Conversation cost/usage is real, not estimated.** `conversation/say` now
  folds each turn's actual provider `usage` into the conversation, so
  `conversation/cost` and `conversation/stats` report the billed token/cost sum.
  When no turn recorded a priced usage, `conversation/cost` returns `nil` rather
  than falling back to a character-count estimate (issue #12).
- **`sort` is type-safe and numerically correct without a comparator.**
  Comparator-free `(sort xs)` previously delegated to an internal tag order, so
  mixed int/float lists sorted every int before every float regardless of value
  (`(sort (list 3 1.5))` → `(3 1.5)`) and heterogeneous lists
  (`(sort (list 3 "a" 1))`) returned a silent, non-portable ordering. Now ints
  and floats compare as one numeric family by value, every other element must
  share its kind, and mixing unrelated types raises a type error pointing at
  `sort-by` / a 2-arg comparator. Deliberate cross-type ordering via an explicit
  comparator is unchanged.
- **`str` is now a real alias of `string-append`.** The two builtins had
  byte-identical but separately copy-pasted implementations; `str` is now
  registered as an alias of `string-append` (matching how `string/append`
  already works). Behavior is unchanged — `str`, `string-append`, and
  `string/append` remain interchangeable.
- **`path/within?` could be fooled by a symlink.** For a path that doesn't exist
  yet (the "agent about to write a new file" case) it now resolves the deepest
  existing ancestor — so a symlink inside the sandbox can't escape it — and no
  longer rejects legitimate paths under a symlinked prefix (e.g. macOS
  `/var`→`/private/var`).
- **`term/strip` swallowed text after non-SGR sequences.** It now parses full CSI
  and OSC sequences (cursor moves, clears, titles), not just colors.
- **Arrow/navigation keys could leak as literal characters.** `io/read-key` reads
  unbuffered from the raw fd so escape-sequence continuation bytes stay visible to
  `select()`, fixing intermittent `^[[C`-style leakage under fast terminals.
- **Streamed tool calls were dropped (Anthropic).** `stream_complete` now
  assembles `tool_use` blocks into `tool_calls`, so a streaming `agent/run` turn
  can call tools.
- **Multi-turn tool history failed on re-send.** `agent/run`'s `:messages`
  round-trip now preserves assistant `tool_calls` and the tool-result
  `tool_call_id`/name, so a conversation that used a tool no longer errors on the
  next turn (e.g. Anthropic `tool_use_id` validation).
- **Archive extraction could silently overwrite.** `zip/extract` / `tar/extract`
  reject two entries that map to the same target; `diff/apply` folds hunk drift
  into its offset; `sys/which` honors `PATHEXT` on Windows.
- **`sema/check-string` / `sema/check-file` dropped diagnostic detail.** A reader
  error carrying a hint was wrapped, so the result reported a generic `:code
  "error"` with no `:span`; it now classifies the root error and returns `:code
  "syntax"` with the `:span` — important for agent repair loops.

### Docs and Website

- **Consolidated duplicate builtin doc entries.** Thirteen builtins
  (`odd?`, `even?`, `zero?`, `positive?`, `negative?`, `=`, `vector?`,
  `record?`, `first`, `rest`, `nth`, `length`, `assoc`) each had two doc files
  filed under different modules. Because LSP hover and REPL apropos index by
  name, only one of each pair was ever surfaced while the other silently drifted;
  each is now a single entry with the richer content merged in. The merged
  `assoc` entry documents both its map-update and association-list forms.
- **Clarified `sys/os` vs `sys/platform`.** Docs (builtin index and website)
  now explain that `sys/os` returns the raw, open-ended OS name
  (`"ios"`/`"android"`/`"freebsd"`/…) while `sys/platform` normalizes to the
  closed set `"macos"`/`"linux"`/`"windows"`/`"unknown"`.

### Repository

- **Editor plugins split into their own repos.** The editor plugins and the
  tree-sitter grammar moved out of the monorepo into dedicated repositories under
  the [`sema-lisp`](https://github.com/sema-lisp) org, each with its own CI and
  publishing: `vscode-sema`, `zed-sema`, `intellij-sema`, `emacs-sema`,
  `helix-sema`, `sema.nvim`, `sema.vim`, `tree-sitter-sema`, and a new
  `sublime-sema` (Sublime Text support). Beyond syntax highlighting, several
  plugins now wire up Sema's built-in language server (`sema lsp`), debugger
  (`sema dap`), and MCP server (`sema mcp`). The `editors/` tree and its
  build/publish workflows were removed from this repo, and the editor docs
  (`README`, [sema-lang.com/docs/editors](https://sema-lang.com/docs/editors))
  now point at the org repos.

## 1.28.1

### Fixed

- **Dynamic workflow resume is safer.** Checkpoint writes now resume lazily:
  when a checkpoint memo exists, `(checkpoint :key expr)` returns the stored
  value without evaluating `expr` again. Checkpoint journal events also carry
  the resume `content_key`, so checkpoint memo files can be inspected and
  invalidated with the same model as agent leaves.
- **Workflow memo keys now include workflow source and `--args`.** Editing the
  workflow or changing run arguments invalidates stale memo hits automatically,
  while unchanged leaves still resume per-leaf.
- **Workflow-declared sandbox permissions are enforced.** `defworkflow`
  metadata can declare `:permissions` using the same syntax as `--sandbox`
  (`strict`, `all`, `none`, or comma-separated denial capabilities such as
  `no-fs-write,no-network`). Workflow permissions can only tighten the
  caller's sandbox; they cannot loosen a stricter CLI sandbox or
  `--allowed-paths` setting.
- **Crates.io publish order includes `sema-workflow`.** The publish workflow
  now publishes the workflow runtime crate before crates that depend on it.

### Docs and Website

- **Workflow documentation caught up with the runtime.** The workflow guide,
  agent-facing docs, CLI sandbox reference, builtin docs, changelog, and
  deferred notes now document `:permissions`, checkpoint resume behavior,
  memo invalidation, and the complete workflow permission list. The abbreviated
  `:perms` metadata spelling is not documented or accepted.
- **Website rendering fixes.** The notebook feature page has more stable
  shortcut layout, the website logo color is fixed, and generated Open Graph
  images are deterministic by using vendored fonts and blocking external font
  requests during generation.
- **Docs/site maintenance.** Architecture docs now include the `sema-workflow`
  crate, the workflow docs have an Open Graph image, and the playground loading
  screen has a small rotating Lisp-joke set.

## 1.28.0

### Added

- **7 new chat/inference providers.** DeepSeek (`DEEPSEEK_API_KEY`),
  OpenRouter (`OPENROUTER_API_KEY`), Together AI (`TOGETHER_API_KEY`),
  Fireworks AI (`FIREWORKS_API_KEY`), Cerebras (`CEREBRAS_API_KEY`),
  SambaNova (`SAMBANOVA_API_KEY`), and Perplexity (`PERPLEXITY_API_KEY`).
  All are OpenAI-compatible and auto-configured from env vars.
  `llm/configure`, `llm/auto-configure`, and `llm/with-fallback` support them.
- **3 new embedding/reranking providers.** Nomic (`NOMIC_API_KEY`),
  Together AI, and Fireworks AI now support embeddings and reranking via
  `llm/configure-embeddings` and `llm/auto-configure`. New `RerankDialect`
  variants for each provider's wire format.
- **`:int` type validation in `llm/extract`.** The `:int` type tag is now
  properly validated — accepts integers and whole-number floats, rejects
  other types.
- **`[:type]` list syntax in `llm/extract` schemas.** A field spec like
  `{:authors [:string]}` is now sent to the model as "array of string" and
  validated as a list where each element matches the inner type.

### Fixed

- **`llm/configure-embeddings` now wires reranking.** Previously, configuring
  embeddings via `llm/configure-embeddings` for Jina, Voyage, or Cohere did
  not call `.with_rerank()` or `set_rerank_provider()`, so `llm/rerank` would
  fail with "does not support reranking". All three providers now correctly
  set up reranking when configured via this path.
- **`llm/extract` and `llm/classify` are now sandbox-gated with `Caps::LLM`.**
  Previously only `llm/extract-from-image` was gated. Text-based extraction
  and classification made LLM API calls even when the sandbox denied `Caps::LLM`.
- **Dynamic workflows.** `defworkflow` / `phase` / `step` / `checkpoint` /
  `parallel` / `pipeline` — a journaled, resumable agentic-workflow runtime.
  Define multi-phase LLM workflows as ordinary Sema code; the runtime journals
  every event to a frozen JSONL run directory (`.sema/runs/<run-id>/`), enforces
  budget caps (`:tokens` / `:usd`), and supports `--resume` via content-keyed
  memo sidecars. `sema workflow run` / `view` / `index` / `check` CLI commands.
  A web viewer (`sema workflow view`) renders live runs with a SQLite cross-run
  index. `sema workflow check` statically validates workflow files without
  evaluating them — catches arity traps and layout issues before a run.
  Doc entries for all workflow builtins are available in LSP hover/completion.
  Feature page at [/feature/workflows](https://sema-lang.com/feature/workflows).
- **`:stack-trace` on caught error maps.** The VM now captures the call stack
  at error time and serializes it as a `:stack-trace` field on caught error
  maps — a list of `{:name :file :line :col}` frame maps, innermost first.
  For inline opcodes (`+`, `-`, `car`, etc.), a synthetic intrinsic frame is
  synthesized by decoding the opcode at the failing PC. TCO-bounded: tail
  calls reuse frames, so the trace stays small even for deep recursion.
  Source spans are now threaded through the main eval path
  (`run_exprs_on_vm`) via `compile_program_with_spans_and_natives`, so
  function frames carry `:line` and `:col` from the original source.

### Fixed

- **`pretty_print` no longer double-serializes values.** `pretty_print` called
  `format!("{value}")` to check if the compact form fit, then `pp_value` repeated
  the same `format!` at `indent=0` when it didn't — walking and stringifying the
  entire tree twice. The redundant check is removed; `pp_value` handles it
  directly. Affects every REPL result, DAP variable inspection, and WASM
  playground output.
- **Runtime `/` now matches the constant folder for large integers.** The
  optimizer used exact `i64` division while the runtime converted to `f64`
  first, causing `(/ 9007199254740993 1)` to return different results depending
  on whether the operands were compile-time constants. A two-integer fast path
  in the runtime `/` uses exact `i64` division when `a % b == 0`, falling back
  to `f64` for non-whole results. `+`, `-`, `*` were unaffected (both paths
  already used `i64`).
- **`ValueViewRef` — zero-refcount trait impls.** `PartialEq`, `Hash`, `Ord`,
  `Display`, and `pp_value` previously used `view()` which calls
  `Rc::increment_strong_count` + `Rc::from_raw` (and a matching decrement on
  drop) for every heap-typed `Value` comparison. A new `ValueViewRef<'a>` enum
  and `view_ref()` method use `borrow_ref` (raw pointer deref) instead,
  eliminating refcount mutations on every `==`, `cmp()`, `hash()`, and
  `format!()`. Micro-benchmarked **40–60% faster** on `eq`/`cmp` for strings,
  lists, and maps; **39.9% faster** total across the benchmark suite.
- **VM arithmetic and comparison functions use `view_ref()`.** `vm_add`,
  `vm_sub`, `vm_mul`, `vm_div`, `vm_eq`, `vm_lt` (the handlers for every
  `ADD`/`SUB`/`MUL`/`DIV`/`EQ`/`LT`/`GT`/`LE`/`GE` opcode) and the stdlib
  `comparison.rs`, `arithmetic.rs`, `predicates.rs`, `math.rs`, `map.rs`, and
  `list.rs` modules were migrated from `view()` to `view_ref()`, eliminating
  refcount churn on numeric and collection operations.
- **`filter` no longer double-clones passing items.** Each item that passed the
  predicate was cloned twice (once for the predicate call, once for the result
  vector). Now cloned once and reused for both.
- **Optimizer `extend_shadowed` avoids allocation when nothing is shadowed.**
  The function previously called `current.to_vec()` on every `let`/`let*`/
  `letrec`/`lambda`/`do`/`try` form, even when none of the new names were
  foldable builtins (the common case). Now returns `Cow::Borrowed` when no
  foldable names are added, avoiding the allocation entirely.
- **DAP `serde_json::to_string().unwrap()` replaced with error handling.** Four
  sites in the DAP server (event send, initialized event, `send_response`,
  `send_error`) now log serialization errors to stderr instead of panicking.
  A malformed debug value would have crashed the entire debug session.
- **DAP `decode_percent` off-by-one fixed.** The check `i + 2 < bytes.len()`
  required at least one byte after the two hex digits, so a percent-encoded
  sequence at the very end of a file URI (e.g. `file%20` for a trailing space)
  was not decoded. Fixed to `i + 3 <= bytes.len()`.
- **DAP debug query boilerplate extracted into helpers.** The
  `sync_channel(1)` + `send` + `spawn_blocking` + `recv` pattern duplicated
  6× across `setBreakpoints`, `stackTrace`, `scopes`, `variables`, `evaluate`,
  and `setVariable` is now `send_cmd_and_recv` / `send_cmd_and_recv_result`.
- **Formatter `token_text` returns `Cow<str>` and `token_width` avoids
  allocation.** `token_text` was `-> String`, allocating on every call including
  in `measure_width` where only `.len()` was needed. Now `-> Cow<'_, str>`
  (symbols — the most common token — return `Cow::Borrowed`), with a separate
  `token_width` function that computes the width without allocating.
- **Formatter `format_top_level` O(n²) → single-pass.** When alignment failed
  for a group of N consecutive defines, the code formatted only the first and
  re-scanned the remaining N-1 on the next iteration. Now formats the entire
  group in one pass and advances past it.
- **`Span::contains` and `Span::contains_pos` consolidated.** Three
  near-identical span containment functions duplicated across
  `sema-lsp/helpers.rs` and `sema-lsp/scope.rs` are now methods on `Span` in
  `sema-core`, available to all crates.

### Performance

- **`builtin_index()` cached with `OnceLock`.** The ~11K-line JSON doc index
  was deserialized from scratch on every `,apropos` REPL command. Now parsed
  once and cached; subsequent calls return a `&'static DocIndex` reference.
- **`BuiltinDocs::load()` shares entries via `Rc<DocEntry>`.** Each `DocEntry`
  (with full markdown body, params, examples) was cloned once per alias. Now
  all names for the same entry share a single `Rc<DocEntry>`, eliminating
  hundreds of string clones at LSP startup.
- **`expand_query` O(n²) → O(n) dedup.** The synonym expansion in MCP
  `docs_search` used a linear scan per token for de-duplication. Now uses a
  `HashSet`.

## 1.27.1

### Added

- **Scientific / exponential number literals.** Floats can now be written as
  `<mantissa>e<exponent>` — `6.022e23`, `1.0e19`, `1e-9`, `-2.5E6` (`e`/`E`, optional
  exponent sign, bare-integer mantissa allowed). Out-of-range magnitudes follow
  IEEE-754 (`1e400` → `inf`). An `e`/`E` not followed by (an optional sign and) digits
  is left untouched, so identifiers like `e` and `exp` are never mis-parsed.

### Fixed

- **Breakpoints fire inside async tasks** — in both the **native DAP** (VS Code) and the
  **WASM playground** debuggers. A breakpoint on a line that runs only inside an
  `async`/`async/spawn` task (or via `async/map`/`pool-map`/`async/all`/channels) was
  silently skipped because the scheduler ran every async task step in non-debug mode.
  STOP + CONTINUE and step into/over/out now work at async breakpoints (verified e2e in
  the playground). Known follow-ups: stepping *across* the scheduler into sibling tasks,
  and variable-panel scoping to the paused task's frame at a cooperative stop.
- **stdlib semantic-correctness sweep.** A pass over divergence bugs across the standard
  library: `string/foldcase` and `string-ci=?` now use full **Unicode case folding**
  (`"Straße"` folds to `"strasse"`, so caseless comparison matches `"STRASSE"`) — distinct
  from `string/lower`; plus correctness fixes in equality (`eq`), `shell`, date/time,
  typed arrays, and text helpers, with matching doc updates.

## 1.27.0

### Added

- **Concurrent I/O — blocking leaves now overlap on the scheduler.** Previously
  `async/spawn` could interleave channels and sleeps, but any task that hit network or a
  subprocess froze the single VM thread for the whole round-trip, so spawned I/O ran
  serially. Now `http/*`, `shell`, `llm/embed`, and `llm/complete` / `llm/classify` /
  `llm/extract` **yield to the scheduler** while their work runs on a background runtime
  (via a new cooperative `AwaitIo` yield), so spawning them as tasks makes wall-clock
  approach `max(latency)` instead of `sum(latency)`. Verified live: 4× concurrent
  `llm/complete` ~3.4× faster than serial; 4× `llm/embed` ~13.6×; 5× `shell` 514 ms vs
  2571 ms. Top-level (non-async) calls are unchanged — byte-identical synchronous behavior.
- **`async/pool-map`** — bounded-concurrency fan-out: `(async/pool-map f items n)` maps `f`
  over `items` with at most `n` calls in flight, results in input order. Fan a large batch
  (embeddings, fetches, completions) across a rate-limited resource without launching
  everything at once.
- **`async/map` and `async/spawn-all`** — ergonomic unbounded fan-out. `(async/map f items)`
  is a concurrent `map` (a task per item, results in input order); `(async/spawn-all thunks)`
  spawns a list of zero-arg thunks and awaits them all. Both are the obvious sugar over
  `(async/all (map #(async/spawn …) …))`; reach for `async/pool-map` when you need a cap.
- **Nested-trace propagation across `async/spawn`.** Spans opened inside a spawned task now
  nest under the spawning task's active span and share its trace, so
  `(with-span … (async/map …))` (or any nested async) renders as ONE connected trace tree in
  Jaeger/Phoenix/Langfuse instead of fragmenting into N disconnected single-span traces. A
  top-level spawn (no active span) still starts its own trace, so independent top-level tasks
  stay isolated.
- **True cancellation.** `async/cancel` and `async/timeout` now **abort in-flight I/O** for
  real where the runtime allows: a cancelled/timed-out `http/*` request tears down its
  connection, and a `shell` subprocess is **killed** (`SIGKILL`) instead of running to
  completion in the background. `llm/*` cancellation stays best-effort (the blocking worker
  can't be interrupted mid-call; the result is discarded). `async/timeout` expiry now
  cancels its target task automatically (you no longer need a paired `async/cancel` to free
  its resources).
- **Per-task OpenTelemetry isolation.** Concurrent LLM tasks each carry their own span
  stack + conversation/session/user scope (swapped on every scheduler task-switch), so
  overlapping `llm/embed` / `llm/complete` spans never cross-contaminate.

### Fixed

- **Scheduler no longer reaps still-pending tasks at an outermost exit.** A task spawned in
  one top-level form and awaited in a later one (e.g. a streaming-pipeline collector spawned
  before an `(async/all …)` of the other stages) was being cleared between scheduler runs
  → "async/await: still pending after scheduler run". The reap is now terminal-only.

Docs: [Concurrency → Concurrent I/O](https://sema-lang.com/docs/stdlib/concurrency).

## 1.26.0

### Added

- **Sema-native tracing API.** New `otel/*` builtins + `with-span` / `with-session` macros
  let your *own* Sema code emit first-class spans — not just the auto-instrumented `llm/*` /
  `agent/*` paths. `with-span` / `otel/span` wrap a block in a generic span;
  `otel/set-attribute(s)`, `otel/set-status`, and `otel/event` annotate the innermost active
  span; the typed helpers `otel/llm-span` + `otel/llm-usage`, `otel/tool-span`, and
  `otel/retrieval-span` render a user-built LLM/tool/retrieval step as an LLM/TOOL/RETRIEVER
  span in Phoenix/Traceloop/Langfuse (via the `SEMA_OTEL_COMPAT` layer) with `gen_ai.usage.*`
  accounting identical to the built-ins; `with-session` / `otel/with-session` groups
  non-agent code into Langfuse Sessions/Users. Every form is a no-op when tracing is off and
  never changes a program's return value. Docs: [Tracing & Metrics → Adding your own
  spans](https://sema-lang.com/docs/llm/observability). Test: `crates/sema/tests/otel_native_test.rs`.

## 1.25.0

### Added

- **`llm/stream` now applies resilience at stream-open.** Streaming was bypassing the whole
  dispatch layer; it now runs through **rate-limiting** and **provider fallback** before the
  first token (a primary that fails to *open* the stream fails over to the next; once a token
  is delivered, a mid-stream failure surfaces and keeps the partial — failing over would
  re-emit it). Budgets can gate streams with `llm/with-budget {... :on-stream :pre-gate}`
  (off by default). The cache and mid-stream retry still don't apply to streams (use
  [cassettes](https://sema-lang.com/docs/llm/cassettes) for deterministic stream replay).
  Verified live (OpenAI bad-model → fail over to Anthropic mid-`llm/stream`).
- **Per-call `:timeout` (ms)** on `llm/complete` / `llm/chat` / `llm/send`. The option
  now reaches the HTTP layer as a per-request reqwest timeout for the network providers
  (Anthropic / OpenAI / Gemini) — previously parsed but ignored. (Local Ollama is excluded;
  streaming calls aren't capped, since a wall-clock timeout would kill a long legitimate stream.)

### Fixed

- **OpenAI streaming dropped tool calls.** `stream_complete` returned an empty `tool_calls`,
  discarding tool-call deltas — streaming agents on OpenAI were broken. It now accumulates the
  index-keyed `id` / `function.name` / `function.arguments` fragments and assembles them into
  the final response (verified live against the OpenAI API).
- **Gemini silent empty output.** A thinking model with a small `:max-tokens` could spend the
  whole budget reasoning and return an empty string with `finishReason: MAX_TOKENS` (exit 0,
  no signal). It now raises an actionable error telling you to raise `:max-tokens` / lower
  `:reasoning-effort` (verified live; normal calls unaffected).

## 1.24.0

### Added

- **Stdlib ergonomics — routine text/list/number helpers.** Added seven builtins that
  everyday code kept hand-rolling: `math/round-to` (round to N decimals) and
  `math/format-fixed` (fixed-decimal display string); `string/lines` (split on line
  endings, Clojure `split-lines` semantics); `list/contains?` (boolean membership, vs
  `member`'s Scheme tail), `list/nth-or` (safe indexed access with a default), and
  `list/take-last` / `list/drop-last` (tail counterparts to `take`/`drop`). The RAG
  example now leans entirely on stdlib (`list/chunk`, `flat-map`, `string/take`,
  `math/round-to`) instead of local helpers.

### Changed

- **Eval test macros emit one test per case.** With the tree-walker retired, `eval_str`
  and `eval_str_compiled` are the same path, so `eval_tests!` / `eval_error_tests!` no
  longer generate redundant `_tw` + `_vm` pairs — halving the eval test count with no
  loss of coverage.

- **Reranking + a full RAG pipeline.** New `llm/rerank` cross-encoder reranking over
  Cohere / Jina / Voyage (the same key already used for embeddings) — `(llm/rerank query
  documents {:top-k 5 :model "..." :provider :cohere})` returns `{:index :score :document}`
  maps, highest relevance first. This completes the retrieve-many → rerank-to-a-few RAG
  recipe with `llm/embed` + `vector-store/*` + `llm/complete`. The vector-store search and
  rerank steps emit OpenInference `RETRIEVER` / `RERANKER` spans (`retrieval.documents.*`,
  `reranker.*`) so a full RAG trace renders natively in Phoenix/Arize. New worked example
  `examples/llm/rag-docs-search.sema` (indexes Sema's own docs; `make rag-demo`), a
  [RAG guide](https://sema-lang.com/docs/llm/rag), and a FakeProvider regression test.

- **OpenTelemetry tags, metadata & streaming time-to-first-token (compat layer).** With a
  `SEMA_OTEL_COMPAT` mode on, every LLM span is now auto-tagged with
  `operation:`/`provider:`/`model:` (+ `cache-hit`), and you can pass `:tags` (a list) and
  `:metadata` (a map) to `llm/complete`, `llm/chat`, `llm/stream`, and `agent/run` — tags
  merge with the auto-tags, metadata fans out to each backend's native field
  (`langfuse.trace.metadata.*`, `langsmith.metadata.*`, `traceloop.association.properties.*`,
  `braintrust.metadata`). Streamed calls record **time-to-first-token**
  (`sema.gen_ai.server.time_to_first_token` always-on; Langfuse `completion_start_time` +
  Traceloop `gen_ai.is_streaming` under compat) — a signal almost no other emitter
  reports. LangSmith now also gets its own `langsmith.trace.session_id`, and
  Langfuse a `langfuse.release` from `SEMA_OTEL_RELEASE`. Verified end-to-end against a live
  OTel Collector (HTTP + gRPC) and Jaeger with real provider calls; regression test in
  `crates/sema/tests/otel_tags_test.rs`.
- **OpenTelemetry per-direction cost split & embedding detail (OpenInference compat).** LLM
  spans now also carry `llm.cost.prompt` / `llm.cost.completion` next to `llm.cost.total`
  (so Phoenix/Arize show the prompt-vs-completion cost breakdown), and embeddings spans
  carry `embedding.model_name` plus (content-gated, capped) `embedding.embeddings.{i}.embedding.text`.

### Documentation

- **Website docs audit + reference coverage.** Documented the nine new builtins
  (`math/round-to`, `math/format-fixed`, `string/lines`, `list/contains?`, `list/nth-or`,
  `list/take-last`, `list/drop-last`, `io/read-line`, `io/eof?`) on their stdlib pages,
  added the OTel cost-split/embedding-detail attributes to the compat doc, and gave the
  RAG/rerank guide a depth pass (score semantics, top-k, cost/scaling, error handling,
  observability). Fixed copy-paste examples that didn't run: `shell` returns a map
  (`:stdout`/`:stderr`/`:exit-code`), the web-server demo's streaming/summarize/extract
  handlers (`llm/stream`, `llm/complete`, and `llm/extract`'s schema-first argument order).

## 1.23.0

### Added

- **LLM cassettes — record/replay for deterministic, keyless testing & demos.** A new
  `sema-llm::cassette` layer records real LLM responses to an NDJSON tape once, then
  replays them deterministically forever — no API key, no network. `llm/complete`,
  `llm/chat`, `llm/extract`, **agent loops** (`agent/run`, each turn keyed
  independently), **streaming** (`llm/stream`, the chunk sequence is recorded and
  replayed in order) and **embeddings** (`llm/embed`, vectors recorded and replayed)
  are all covered. Modes
  `:auto` / `:replay` / `:record`; a `:replay` miss is a hard error that surfaces
  prompt drift. Surface: `(llm/with-cassette path opts thunk)`,
  `llm/cassette-load`/`-save`/`-eject`, and `SEMA_LLM_CASSETTE` /
  `SEMA_LLM_CASSETTE_MODE` for CI. Folds with the rest of the runtime: it sits below
  the OpenTelemetry span + response cache + cost accounting and above the provider, so
  a replay still emits its `chat` span and reports its **recorded** usage (distinct
  from a cache hit's zero usage), and `with-cassette` disables the response cache for
  its scope. The tape stores only the response keyed by a request hash — no prompt
  text, key, or header touches disk (redaction by construction). Docs:
  `website/docs/llm/cassettes.md`.

### CI

- **Publish-list guard.** `scripts/check-publish-list.sh` (run in the release
  `verify` gate + `make check-publish-list`) fails if a publishable workspace crate is
  missing from `publish.yml`'s order — preventing the half-published release that hit
  1.22.0 when the new `sema-otel` crate wasn't in the list.
- The two publishes now share a single `verify` gate (was one full suite per
  registry), CI uses `Swatinem/rust-cache`, and the per-crate publish sleeps are
  trimmed (cargo already waits for index propagation).

## 1.22.0

### Added

- **OpenTelemetry observability (opt-in, GenAI semantic conventions).** A new
  `sema-otel` crate emits standards-compliant traces + metrics for every LLM/agent
  run, exportable to any OTLP backend (Jaeger/Langfuse/Datadog/Grafana/Honeycomb/
  Phoenix) or a JSONL file — consumed natively by `gen_ai.*`-aware tools. **Off by
  default and zero-cost when off**; a down/slow collector can never block, add latency,
  or crash a script (thread-based batch processor, bounded queue, drop-on-full, bounded
  flush+shutdown). Coverage: one `chat {model}` CLIENT span per non-streaming
  completion (provider, request/response model, input/output + cache tokens, finish
  reason, `gen_ai.usage.cost_usd`, `gen_ai.cache.hit` on cache hits); `embeddings`
  spans; the full `invoke_agent → (chat, execute_tool {name}, chat)` agent tree;
  per-HTTP-retry child spans; a streaming-call span; and a notebook "Run All" trace
  (one root, one child per cell). Two GenAI metric histograms
  (`gen_ai.client.token.usage`, `gen_ai.client.operation.duration`). Prompt/response
  **content capture is OFF by default** (opt in via
  `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true`). New Sema builtins
  `(otel/span name thunk)` and `(otel/event name attrs-map)`. Enabled via standard
  `OTEL_*` env vars or `SEMA_OTEL_FILE`; both OTLP transports (HTTP + gRPC) compiled
  in. Embedded hosts keep ownership: Sema never installs a global provider on its own
  and nests under the host's current span. wasm builds compile the facade out to a
  no-op. Docs: `website/docs/llm/observability.md`.
  - **Production hardening (post-MVP):** gRPC OTLP export now actually works (was a
    latent no-op — the tonic exporter ran on the static async runtime); spans/metrics
    carry `gen_ai.conversation.id` plus `session.id` / `user.id` so multi-turn runs
    group into **Langfuse Sessions** (`:conversation-id` / `:session-id` / `:user-id`
    options on `agent/run`, `llm/chat`, `llm/complete`); message content is captured in
    the structured GenAI JSON shape; streaming + embeddings spans now record errors;
    `gen_ai.output.type` is emitted; and the instrumentation scope + Resource are
    enriched (schema URL, service version, runtime). Verified live against a
    self-hosted Langfuse via an OTel Collector (HTTP + gRPC).

- **Backend compatibility layer for LLM-observability tools (`SEMA_OTEL_COMPAT`).**
  Sema always emits the standard `gen_ai.*` GenAI conventions; setting
  `SEMA_OTEL_COMPAT` to one or more *compatibility modes* makes it *also* write a
  tool's own attribute names, so backends that key off their own namespace light up
  fully. Modes: `openinference` (Arize Phoenix / Arize AX), `traceloop` /
  `openllmetry`, `langsmith`, `langfuse`, `braintrust`, and `all`. Purely additive
  (the standard attributes are always present), content-gated detail respects the
  capture opt-in, and spans are typed per tool (`LLM`/`TOOL`/`AGENT`/`EMBEDDING`,
  etc.). Verified live against Phoenix (gRPC + HTTP) and Langfuse. Docs:
  `website/docs/llm/otel-compat.md`, with a fact-checked survey (against each tool's
  own current docs) of which backends accept an OTLP push, which need a mode, and
  which can't receive traces at all.

- **Prompt-cache token reporting across providers.** `llm/last-usage` and `llm/session-usage` now expose `:cache-read-tokens` and `:cache-creation-tokens`, surfacing how many input tokens were served from (or written to) the provider's prompt cache. Wired through every first-party provider for both non-streaming and streaming responses: OpenAI/OpenAI-compatible (`prompt_tokens_details.cached_tokens` — implicit cache), Gemini (`cachedContentTokenCount` — implicit cache on 2.5+), and Anthropic (`cache_read_input_tokens` / `cache_creation_input_tokens`, reported separately from input tokens). Session counters accumulate cache tokens; custom Sema-defined providers can report them via `:cache-read-tokens` / `:cache-creation-tokens` in their usage map. Verified live (OpenAI and Gemini implicit cache reads observed on repeated long prefixes) and with a deterministic FakeProvider regression test. Cached reads are reported for visibility but not yet discounted in `:cost-usd`.

### Changed

- **Unified `sema-llm` onto the canonical evaluator callback.** Tool handlers,
  Lisp-defined provider `:complete` functions, streaming/agent callbacks, and
  `with-*` thunks now run through `sema_core::call_callback` →
  `sema_eval::call_value` (the VM's nested-closure path) instead of a redundant
  hand-rolled application routine. This removes the bespoke `EVAL_FN` / `call_value_fn`
  / `simple_eval` path, so `set!`, captured upvalues, and async/yield *inside* a tool
  handler or callback now share the same VM semantics as standard-library
  higher-order functions. No user-facing API change.

### Docs

- New **[Glossary](https://sema-lang.com/docs/internals/glossary.html)** (178 terms)
  with explicit multi-meaning entries for overloaded words (token, span, agent, tool,
  provider, chunk, atom, …).
- Observability docs gain an **Authentication headers** section (the
  `OTEL_EXPORTER_OTLP_HEADERS` format and per-backend examples), and the architecture
  overview is refreshed (15 crates incl. `sema-otel`; the circular-dependency framing
  corrected).

## 1.21.2

Bugfix + release-process hardening.

### Fixed

- **Special-form names are bindable again.** A change in 1.20.4 reserved special-form names (`if`, `fn`, `let`, `and`, `message`, …) and rejected binding *any* of them — but that also rejected correct value-position use, like a function parameter named `message` or a variable named `fn`, because the scope-free lowerer can't distinguish value use from operator use. It broke real code (5 bundled examples) and slipped a CI regression past four releases. The reservation is removed: special-form names shadow correctly in value position again. In operator/head position the special form still wins (a documented, accepted footgun — `docs/limitations.md` #36); the proper fix (full lexical shadowing) is future work.

### CI / release process

- **Publishing now requires the test suite to pass.** The crates.io and npm publish workflows triggered on a version tag with no dependency on CI, so a red test suite never blocked a release — which is how the reserved-name regression shipped. They now gate on a reusable `verify` workflow that runs the full CI-equivalent suite (fmt, clippy, doc check, `cargo test`, example + bytecode smoke tests) before any publish. The local release runbook was updated to run the example/bytecode smoke tests too (plain `cargo test` skips them).

## 1.21.1

Bugfix. Found by a live stress test of the resilience features (Ollama-down →
Mistral fallback, caching, and budgets).

### Fixed

- **Cache hits double-charged cost and burned budget.** A cached response (`llm/with-cache`) makes no provider call — no tokens are consumed and no money is spent — but its stored token usage was still run through `track_usage`, so `llm/session-usage` cost and `llm/with-budget` spend incremented on every cache hit (e.g. a repeated call reported 2× the cost, and cache hits could trip a budget). Cache hits now report zero usage, so cost/budget reflect actual spend. Verified live (cost unchanged across identical calls; a budget that allows one real call is not tripped by subsequent cache hits) and with a deterministic regression test. Provider fallback and budget enforcement were verified correct and unchanged.

## 1.21.0

LLM/agentic bulletproofing. A multi-agent audit (see `docs/plans/archive/2026-06-21-llm-agentic-audit.md`)
found the headline agent loop broken on OpenAI-family providers and several
robustness gaps; this release fixes them, hardens resilience, and adds
cross-provider reasoning control — all verified live against OpenAI, Anthropic,
and Gemini plus a new keyless deterministic test harness.

### Fixed

- **Agent tool loop was broken on OpenAI-family providers.** Tool results were sent as plain `[Tool result for X]: ...` user text with no `tool_call_id`, and the assistant's `tool_calls` were never echoed — so OpenAI/Ollama models re-called the tool until max-turns and returned an empty response (Anthropic happened to tolerate it). The loop now echoes the assistant `tool_calls` turn and sends correlated tool-result messages, translated to each provider's native shape (OpenAI/Ollama `tool_call_id`, Anthropic `tool_use`/`tool_result`, Gemini `functionCall`/`functionResponse`). Live-verified end-to-end on all three families.
- **gpt-5 / o-series models were entirely uncallable.** They reject the legacy `max_tokens` parameter; the OpenAI serializer now sends `max_completion_tokens` on the official OpenAI/Azure endpoints (compatibility endpoints keep `max_tokens`).
- **Anthropic extended thinking failed to decode.** `thinking` / `redacted_thinking` response blocks caused a hard "error decoding response body"; they're now tolerated.
- **Documentation accuracy:** homepage no longer claims a runtime tree-walker (retired; the VM is the sole evaluator) and the `llm/classify` example matches the real signature; README describes pricing as a bundled models.dev snapshot rather than live "dynamic pricing".

### Added

- **Cross-provider `:reasoning-effort`.** One portable option (`:minimal` `:low` `:medium` `:high` `:none` `:xhigh`) on `llm/complete`, `llm/chat`, and `agent/run`, mapped to each provider's native control: OpenAI `reasoning_effort`, Anthropic extended thinking (budget tokens, with the required `max_tokens`/`temperature` adjustments), Gemini `thinkingConfig`. Unsupported providers/models ignore it. See `docs/llm/completion.md`.
- **Recoverable tool errors + argument validation in the agent loop.** A tool that throws, isn't found, or is called with schema-invalid arguments no longer aborts `agent/run` — the error is fed back so the model self-corrects (bounded by `:max-turns` and a 5-consecutive-error cap). Model-supplied args are validated against the `deftool` parameter schema before the handler runs.
- **Keyless deterministic LLM test harness** (`FakeProvider`) so the LLM/agent paths — including the tool-result protocol, retries, and reasoning mappings — are covered in CI without API keys.

### Improved

- **Resilience now matches the docs.** Network retry covers transient 5xx and network/timeout errors (not just 429) with capped exponential backoff + jitter; 429 honors the server `retry-after`. Non-retryable 4xx fail fast.
- **OpenAI compatibility hardening.** Azure OpenAI endpoints are detected for the gpt-5 parameter conventions, and a self-healing backstop learns per-model when a custom `temperature` is rejected (gpt-5.0 / o-series) and retries without it — so portable code keeps working. Streaming's bypass of cache/budget/fallback is now documented honestly (`docs/llm/resilience.md`).

## 1.20.4

Diagnostics release. Closes a special-form shadowing footgun and adds actionable hints for the most common cross-dialect mistakes.

### Fixed

- **Binding a special-form name silently mis-shadowed in operator position.** Because the bytecode lowerer is scope-free, a local binding whose name collides with a special form (`if`, `fn`, `let`, `and`, `cond`, `define`, `match`, …) could not override that form when used as the head of a call — the special form silently won, so `(let ((and (fn (a b) (* a b)))) (and 3 4))` returned `4` (the `and` special form), not `12`. Special-form names are now **reserved identifiers**: binding one in a `let`/`let*`/`letrec` binding, a `fn`/`lambda`/`defun`/`define` parameter, or a `define`/`defun`/named-`let` name is rejected at the bind site with a clear error (`cannot bind reserved special-form name '...'`). Regular names — including builtin *functions* like `list`/`map`/`filter` — still shadow freely. Matches the Common Lisp / Clojure model; see ADR #65 and `docs/limitations.md` #36.

### Improved

- **Actionable hints for common cross-dialect mistakes.** Type errors now redirect rather than just restating the expected type: `(+ 1 "x")` (mixing strings with other types) suggests `(str a b ...)`; `(get [1 2 3] 1)` / `(contains? [1 2 3] 1)` (Clojure-style vector indexing) suggest `(nth coll i)`; `(nth 1 coll)` (swapped arguments) explains that the order is `(nth collection index)`. Hints are added to both the VM intrinsic paths and the first-class stdlib functions.

## 1.20.3

Bugfix release. Three correctness fixes surfaced by a multi-agent bug-hunt across subsystems the grammar fuzzer can't structurally reach.

### Fixed

- **`get-in` conflated a present `nil` value with a missing key.** `(get-in {:a nil} [:a] "default")` returned `"default"` instead of `nil`, and `(get-in nil [] "default")` returned `"default"` instead of the root `nil`. `get-in` now distinguishes a key that is present (even with a `nil` value) from a missing key, and an empty path returns the root collection (Clojure semantics).
- **IEEE 754 special floats didn't round-trip through the reader.** The printer emits `inf` / `-inf` / `NaN`, but the reader parsed those back as *symbols*, so `(read (str (/ 1.0 0.0)))` was a symbol, not a float. The reader now recognizes `inf` / `-inf` / `NaN` (and common spellings like `+inf`, `Infinity`, `nan`) as float literals.
- **Readable string printing didn't escape special characters.** Strings containing `"`, `\`, newlines, tabs, or carriage returns printed inside a container (or via `str` in readable position) were emitted unescaped, so `(read (str (list "a\nb")))` didn't reproduce the original. The readable form now escapes `\\ \" \n \t \r`; the reader already parsed these back. Bare strings printed via `display`/`println`/`str` use a separate raw path and are unchanged.

## 1.20.2

Bugfix release. Fixes silent integer corruption in the VM's inline add/subtract, found by the grammar fuzzer's new metamorphic oracle.

### Fixed

- **Silent integer corruption: inline `+`/`-` truncated results past ±2⁴⁴ (~17.5 trillion).** The branchless small-int fast paths for the 2-argument inline `ADD_INT`/`SUB_INT` opcodes did raw-bit arithmetic and masked the result to the 45-bit NaN-box payload with no overflow check, so any runtime add or subtract whose result crossed the small-int boundary was silently wrapped/truncated — e.g. `(let ((a 9000000000000)) (+ a a))` returned `-17184372088832` instead of `18000000000000`. They now sign-extend to `i64` and build the result through `Value::int`, which promotes to a boxed integer on overflow (matching `MUL_INT`, which was already correct). Small ints stay unboxed immediates; literal operands were unaffected (constant-folded at compile time), which is why it went unnoticed. Found by the in-language grammar fuzzer's metamorphic distributivity law (`a*(b+c) == a*b + a*c`).

## 1.20.1

Bugfix release. Fixes a VM crash on valid code, found by a new in-language grammar fuzzer.

### Fixed

- **VM crash: a throwing `try`/`catch` as a non-first binding in a parallel `let` corrupted the operand stack.** `(let ((a 1) (b (try (throw 1) (catch e 2)))) b)` aborted the process instead of returning `2`. `compile_let` pushed all binding inits onto the operand stack without updating `stack_height`, so an exception handler inside a later init restored the stack *below* the earlier already-pushed inits, and subsequent local-slot access went out of bounds. Fixed by tracking `stack_height` across the init pushes/stores (the way call-argument compilation already does). `let*`, `letrec`, and calls were unaffected.

### Added

- **Grammar-based fuzzer written in Sema** (`fuzz/grammar-fuzz.sema`, `make fuzz-grammar`). Generates well-typed, closed programs over int/bool/float/string/list/vector/map and checks two oracles — printer⇄reader round-trip and a differential compiler/VM value oracle (expected computed bottom-up while generating) — plus crash detection, all reproducible from a single integer seed. This is what found the `try`/`let` crash above; the VM then ran 715k generated programs (depths 4–9) clean. Documented at `docs/internals/fuzzing`.

## 1.20.0

Tooling and ergonomics release. `match` is now exhaustive-by-default (raises on no-match, with a new lenient `match*`), the LSP gains range formatting, the debugger gains conditional and exception breakpoints, lowering is faster, and the numeric domain policy is now documented. One behavior change to be aware of — see **Changed**.

### Changed

- **`match` now raises on no-match (breaking).** `(match …)` with no matching clause previously returned `nil` silently — a non-exhaustive match is almost always a bug, and the silent `nil` masked it. It now raises `match: no clause matched value: …` (a catchable `:eval` error). Add a catch-all `(_ …)` clause, or use the new `match*` when "no match" is a legitimate outcome.

### Added

- **`match*`** — the lenient counterpart to `match`: returns `nil` when no clause matches (the old `match` behavior), for lookup-style use where a miss is normal.
- **LSP range formatting** (`textDocument/rangeFormatting`). The server now advertises `documentRangeFormattingProvider` and formats a selection by expanding it to the smallest set of *whole* top-level forms it overlaps, formatting those through `sema-fmt`, and returning edits scoped to that span. Formatting partial sub-expressions in a Lisp is unsafe, so a selection that touches no complete form is a no-op.
- **DAP conditional breakpoints and an uncaught-exception breakpoint.** `supportsConditionalBreakpoints` is now on: a breakpoint's `condition` is evaluated (in the stopped frame, via the existing evaluate path) and a pure breakpoint stop only fires when it's truthy; a bad condition fails open so it surfaces. An `uncaught` exception filter (`setExceptionBreakpoints`) stops on errors that escape to the top level, with `exceptionInfo` reporting the message. (At an uncaught stop the VM has already unwound, so stack/scopes there are best-effort — the message is the load-bearing info.)

### Documentation

- **Numeric domain & error policy is now documented** (ADR #64, `stdlib/math.md`). No behavior change — the existing rule is ratified: integer division/modulo by zero raises (integers have no `inf`/`NaN`), while all floating-point operations follow IEEE 754 (`sqrt -1` → `NaN`, `1.0/0` → `inf`, `log 0` → `-inf`, …). Integer overflow wraps (no bignums yet).

### Performance

- **Special-form lookup cache in the bytecode lowerer.** `lower_list` previously checked every list form against ~40 special-form names, each check re-interning the name (a thread-local `RefCell` borrow + hashbrown lookup per name per form). It now does a single lookup into a per-thread `HashMap<Spur, SpecialForm>` and matches on the resulting enum. Measured **~1.2× faster lowering** on a 10k-form file (88.6 ms → 73.6 ms); emitted bytecode is byte-identical. (Per-thread, not global, because the interner is thread-local.)

## 1.19.2

Performance release. No language or behavior changes — purely faster, fully backward compatible. The headline is **profile-guided optimization** of the distributed binaries.

### Performance

- **PGO'd release binaries.** The cargo-dist GitHub-release binaries and Homebrew bottle are now built with profile-guided optimization (instrument → train on the benchmark suite + a 1BRC sample → rebuild). Measured **~25–29% faster on the 1BRC workload** and **−11% to −40% across the compute benchmarks** (higher-order-fold −40%, tak −32%, deriv/hashmap −22%). Wired into CI via dist's `github-build-setup` on native runners; it falls back to a plain build on cross-compiled targets and is fail-safe (a PGO failure never fails the release). Run it yourself with `make build-pgo`. Note: `cargo install` builds get fat LTO but not PGO (PGO needs the training step).
- **Fat LTO** for release/dist profiles (`lto = "fat"`): measured 3–9% on its own, independent of PGO.
- **Inline string opcodes.** `string-length`, `string-ref`, and `string-append` (2-arg) now compile to dedicated VM opcodes instead of routing through the generic `CallGlobal` → hash-lookup → native-fn path. Semantics are identical (char-indexed, same errors, redefinition still falls back); a win for code that uses them in hot loops.
- **`#[inline(always)]`** added to the `type_name`/`as_str` value accessors used in the VM dispatch loop.

Bytecode is unchanged at `format_version` 4 (the new opcodes are additive and single-byte); `.semac` files remain version-pinned as before.

## 1.19.1

Dependency-hygiene patch release. No functional changes — this exists so the cargo-dist GitHub-release binaries and the Homebrew bottle (which build from the committed `Cargo.lock`, unlike `cargo install`, which re-resolves) embed the security-patched `tar` crate.

### Fixed

- **`tar` bumped 0.4.45 → 0.4.46** (RUSTSEC / GHSA-3pv8-6f4r-ffg2, PAX-header desync, medium severity). Used by the `sema build` standalone-executable archive path. `cargo install` users already resolved the patched version via `tar = "0.4"`; this pins it into the distributed binaries and Homebrew.
- **`esbuild` bumped 0.28.0 → 0.28.1** in the playground and `ui` dev toolchains (GHSA-g7r4-m6w7-qqqr, dev-server file read on Windows, low severity, dev-only — we use esbuild for bundling, not serving).

## 1.19.0

Real `async/sleep` in the browser playground, plus cancellation and a revived runaway-loop guard. The WASM playground now runs evaluation on a dedicated Web Worker that blocks on `Atomics.wait`, so `async/sleep` paces in real wall-clock time while the page stays responsive — and a Stop button can cancel a running program. Also fixes debugging async programs in the playground, and stops shipping the internal `sema-docs` binary via Homebrew.

### Added

- **Cancellation / interrupt API.** `sema_core::set_interrupt_callback(fn() -> bool)` / `check_interrupt()` let a host abort a running evaluation; the VM polls it at loop back-edges and the async scheduler checks it between task steps (clearing pending tasks on cancel). New `set_blocking_sleep_callback` / `blocking_sleep_ms` let a host supply real wall-clock pacing for `async/sleep` (the playground worker uses `Atomics.wait`; native uses `std::thread::sleep`; the default in WASM is an instant virtual-clock advance).
- **Web Worker eval path in the playground** (real `async/sleep`, responsive UI, a working Stop button, synchronous-XHR HTTP, and **live-streamed `println` output** so a long-running/sleeping program shows output as it happens instead of all at once at the end). Active when the page is cross-origin isolated; opt out with `?no-worker`. Internal/playground-only (`sema-wasm` is not published to crates.io), but the supporting `sema-core`/`sema-vm` hooks above are part of the public API.

### Fixed

- **Runaway loops are bounded again.** `eval_step_limit`/`eval_steps` were dead leftovers from the retired tree-walker and were never checked by the VM, so an embedder that set a step limit got no protection (and the playground's 10M-step "guard" was inert — a tight infinite loop could hang). The VM now enforces the step limit at loop back-edges and tail-call transitions, via a single guard that also honors the wall-clock deadline and cancellation. A/B-benchmarked (tak): no measurable overhead.
- **`async/timeout 0` no longer trips before synchronously-ready work runs** — it only fires once the virtual clock actually reaches the deadline with the task still pending.
- **Debugging async programs in the playground no longer errors** with "async/spawn: no async scheduler registered." The WASM `debugStart` path now initializes the async scheduler, like the normal eval path and the native DAP server.
- **`sema-docs` is no longer shipped as a binary** via cargo-dist / Homebrew (`[package.metadata.dist] dist = false`). It is an internal doc-generation tool (`make docs`); the auto-generated Homebrew formula now installs only the `sema` binary. (`dist plan` confirms a single `[bin] sema` per artifact.)
- **CI: dropped stale tree-walker smoke steps** (`make examples-vm` was removed) and skipped the LLM-provider example in the deterministic example smoke run.
- **No more `profile package spec 'sema-wasm' ... did not match any packages` warning** on `cargo install`/`cargo build -p sema`. The wasm size optimization (`opt-level = "s"`) moved out of the workspace `[profile.release.package.sema-wasm]` (which warned for any build graph not containing `sema-wasm`) and is now injected only during the wasm build via `wasm-pack ... -- --config` in the Makefile. The playground wasm size is unchanged (~3.18 MB).

### Changed

- **Playground developer experience:** the build emits sourcemaps, and `make playground-dev` generates a Chrome DevTools Automatic Workspace descriptor (`/.well-known/appspecific/com.chrome.devtools.json`) for edit-to-disk against the real source.

## 1.18.0

Tree-walker retirement release. The legacy tree-walking interpreter is gone — the bytecode VM is now Sema's sole evaluator across every entry point (CLI, REPL, embedding API, `eval`, `import`/`load`, macros, async). Ships with refreshed default LLM models, an embedded models.dev pricing snapshot, a hardened standalone-binary build path, dependency bumps, an async example suite, and a runnable Rust embedding example. The VM/tree-walker switcher is removed from the playground.

### Removed

- **The tree-walking interpreter has been retired — the bytecode VM is now Sema's sole evaluator.** Every entry point (the CLI, the REPL, the embedding API, `eval`, `import`/`load`, macros, async/await) compiles to bytecode and runs on the VM. Macro expansion, `import`/`load` (with full module isolation), `(eval …)`, record types, and `deftool`/`defagent` are all VM-native. The `--tw` and `--vm` CLI flags are **removed** (the VM has been the default since v1.14; `--vm` was redundant and `--tw` selected the now-deleted backend). The dead evaluator source (~2,180 lines: `eval_value`/`eval_step`/trampoline/`apply_lambda` and all tree-walker special-form handlers) and the internal backend-selection flag are gone. Behavior change for embedders: all eval entry points now run in the global env (top-level `define`s persist across calls) — use a fresh `Interpreter` for isolation. Known follow-up (see `docs/deferred.md`): the VM does not yet emit stack traces on runtime errors (VM-1).
- **The VM/tree-walker engine switcher is gone from the playground.** With the tree-walker retired there is only one evaluator, so the "Tree / VM" toggle was removed from the playground UI; all code runs on the bytecode VM.

### Added

- **Async example suite.** New runnable examples in `examples/` showcase the cooperative scheduler and channels under load: `async-fan-out`, `async-worker-pool`, `async-race-timeout`, `async-pipeline`, and `async-stress` (hundreds of concurrent tasks + channel fan-in).
- **Rust embedding example + expanded embedding tests.** `cargo run -p sema-lang --example embedding` walks the full embedding surface (eval, persistent defines, prelude macros, `register_fn`, `preload_module` with export restriction, async/await, builder toggles, error handling). The embedding API test suite grew from 4 to 14 tests, covering `register_fn` (including use as a higher-order-function callback and error propagation), `eval(&Value)`, global-env injection, `without_stdlib`/`without_llm` builders, `load_file`, and `async/all` + channels.
- **Per-provider model overrides in `llm/with-fallback` chains.** Chain entries may now be `[provider model]` pairs or `{:provider :model}` maps in addition to bare provider keywords, so a single fallback chain can target a different model per provider (e.g. Opus on Anthropic, GPT-5.5 on OpenAI). A per-provider override wins over any `:model` pinned in the call body, and each provider always receives a model id valid for itself. Bare keywords continue to use the provider's configured default. See the [resilience docs](https://sema-lang.com/docs/llm/resilience.html).
- **Provider-aware cost lookup.** `pricing::model_pricing_for(provider, model)` / `calculate_cost_for(provider, usage)` resolve the price for a model *as served by a specific provider*, so a reseller/gateway listing the same model id at a different rate is priced correctly. Cost tracking now uses the provider that actually served each `llm/complete`/`llm/chat` response; unknown providers fall back to the canonical first-party price.

### Changed

- **`(type (lambda …))` now reports `:lambda`, not `:native-fn`.** VM closures are wrapped as native-fn fallbacks internally; a marker on the wrapper now lets `type`/`type_name` report user functions as `:lambda` (closes deferred item VM-2).
- **Dependency bumps:** `hashbrown` 0.17, `lopdf` 0.41, `reedline` 0.48, `libsui` 0.15, `toml` 1.1, `toml_edit` 0.25, `zip` 8, `rusqlite` 0.40 (`sha2`/`hmac` held at 0.10/0.12 — `digest` 0.11 drops the `LowerHex` impl the hex-encoding paths rely on).
- **Default chat models bumped to current flagships across all providers.** When you don't pass `:default-model` (or pin a `:model`), each provider now defaults to: Anthropic `claude-sonnet-4-6` (was `claude-sonnet-4-5-20250929`), OpenAI `gpt-5.5` (was `gpt-4o`, which is being deprecated), Gemini `gemini-3.5-flash` (was `gemini-2.0-flash`), xAI `grok-4.3` (was `grok-3-mini-fast`), Mistral `mistral-large-latest` (was `mistral-small-latest`), Moonshot `kimi-k2.6` (was `moonshot-v1-8k`), and Ollama `gemma4` (was `qwen3:8b` — Gemma is fast and pragmatic to run locally, even on a Mac). Groq stays on `llama-3.3-70b-versatile` (still current). Override any of these per provider with `:default-model`, globally via `SEMA_CHAT_MODEL`, or per call with `:model`. See the [default models table](https://sema-lang.com/docs/llm/providers.html#default-models).
- **LLM pricing now comes from an embedded [models.dev](https://models.dev) snapshot instead of a runtime fetch from llm-prices.com.** A models.dev-derived snapshot (`crates/sema-llm/src/pricing-data.json`, MIT-licensed data, 2,400+ priced models) is vendored and embedded at build time; refresh it with `make update-pricing` and ship the diff in a patch release. This replaces both the stale hand-maintained hardcoded price table (which had drifted to 2025-01 and priced none of the current flagships) and the runtime fetch + disk cache against a third-party endpoint we don't control. In a freshness/accuracy bake-off against llm-prices.com and LiteLLM, models.dev was the only source correct on all current flagships (gpt-5.5, claude-sonnet-4-6/opus-4-8, gemini-3.5-flash, grok-4.3, mistral-large-latest, kimi-k2.6) and the only one carrying the newest launches. `llm/pricing-status` now reports `"embedded"` with the snapshot's `updated-at` date.
- **Dev/test builds now compile with `debug = "line-tables-only"` (`[profile.dev]` in the workspace `Cargo.toml`).** Debug and test builds previously used Cargo's default `debug = 2` (full variable-level debug info), which made each integration-test binary ~84 MB. With ~12 crates, many test targets (`integration_test`, `dual_eval_test`, `vm_async_test`, `vm_integration_test`, the `zz_probe*` files, …), and Cargo never garbage-collecting stale fingerprint binaries, `target/debug` had ballooned to ~39 GB (175+ binaries over 40 MB, plus an 18 GB incremental cache). Dropping to line-tables-only keeps panic backtraces with file:line intact while cutting per-binary size to roughly a third; the only thing given up is variable-level inspection when stepping the *Rust* interpreter under `lldb` in a plain debug build. **Profiling and benchmarking are unaffected**: `make bench*`/`make bench-1m/10m/100m` build under `[profile.release]`, and `make profile`/`profile-vm` build under `[profile.release-with-debug]` (`debug = true`, `strip = "none"`), neither of which inherits from `[profile.dev]`. There are no Criterion `benches/` harnesses that would compile under the dev profile.

### Fixed

- **`async/sleep` now orders tasks correctly in WASM, and `async/timeout` is deterministic everywhere.** The scheduler moved from real wall-clock `Instant`s to a single **virtual clock**: `async/sleep`/`async/timeout` are measured in logical milliseconds that only advance when every task is blocked, jumping to the nearest pending deadline. Previously WASM (the browser playground) treated `async/sleep` as an immediate no-op, so a task sleeping 50 ms and one sleeping 5 ms both resumed instantly — losing duration ordering. Now a shorter sleep always wakes before a longer one, deterministically, on every platform (instant in WASM, real-time-paced on native — CLI scripts that sleep for rate-limiting still wait). This also unified the previously WASM-specific timeout handling into one code path. Two related fixes shipped alongside: a `0 ms` (or very short) `async/timeout` now lets synchronously-ready work finish instead of tripping pre-emptively, and `async/sleep` durations are capped at 1 day (mirroring `async/timeout`) so an out-of-range sleep can't wedge the scheduler.
- **`sema build` no longer hard-fails on imports that can't be resolved at build time.** The static import tracer now warns and skips an import it can't canonicalize/read or that resolves outside the project/packages dirs (e.g. a module written to `/tmp` at runtime, or a path computed dynamically), instead of aborting the build. A package that is declared-but-not-installed still hard-errors. This makes standalone-binary builds of programs that generate or load modules at runtime work end-to-end.
- **`llm/with-fallback` + response caching no longer sends the wrong provider's model id down the chain.** With caching enabled and no model pinned, the cache layer eagerly filled the request model with the *default provider's* default model before the fallback loop ran, which then bypassed each fallback provider's own per-provider model substitution (so e.g. an Anthropic model id could be sent to OpenAI on fallback). The cache key is now computed without mutating the request that flows into the loop, so per-provider model resolution is preserved whether or not caching is on.

## 1.17.1

MCP server stability fix.

### Fixed

- **MCP server no longer crashes on `compile`, `disasm`, or any `notebook/*` tool.** These handlers build a short-lived `Interpreter` (or notebook `Engine`) that owns LLM-provider Tokio runtimes. Dropping a plain runtime from inside the server's async stdio loop panics ("Cannot drop a runtime in a context where blocking is not allowed"), which under the release build's `panic = "abort"` aborted the whole server process — every subsequent tool call then failed with "connection closed". LLM-provider runtimes are now wrapped in a `BlockingRuntime` whose `Drop` calls `shutdown_background()`, so dropping an interpreter is safe in any context. Covered by an end-to-end regression test that drives the real `sema mcp` binary (`crates/sema/tests/mcp_e2e_test.rs`).

## 1.17.0

Tooling + VM release. Adds a built-in **MCP server** (`sema mcp`) so LLM clients can drive Sema in the host environment, makes the **DAP debugger** usable by default with verified breakpoints, evaluate/setVariable, and richer variable inspection, and runs **`(load …)` on the bytecode VM** so async/channels work in loaded files. The bytecode format moves to **v4** (now stack-verified on load, closing a memory-safety hole for untrusted `.semac`). Hardening across the new MCP/DAP surfaces and the formatter API followed an adversarial review and two rounds of mutation testing.

### Fixed

- **async/await and channels now work when running a compiled `.semac` file and under the DAP debugger.** Those execution paths previously ran the VM without initializing the async scheduler, so any async use failed with "no async scheduler registered"; the scheduler is now initialized consistently with running source.

### Added

- **`(load ...)` runs on the bytecode VM** — when the VM is the active backend, a loaded file's body is now compiled and run on the VM instead of the tree-walker. This makes VM-only features (async/await, channels) work inside loaded files and runs loaded code at VM speed. `(import ...)` remains tree-walked (its module isolation needs lexical env capture the VM does not yet provide; tracked in `docs/plans/2026-06-16-vm-module-loading.md`). Under the DAP debugger, loaded files still run outside the attached debug session, so breakpoints inside them are not hit (a one-time warning notes this).
- **Built-in MCP (Model Context Protocol) server** — `sema mcp` runs a stdio JSON-RPC 2.0 server that lets LLM clients (Claude Desktop, Cursor, Claude Code, …) inspect, compile, format, evaluate, disassemble, and build Sema code in the host environment. Ships core developer tools (`run_file`, `compile`, `eval`, `docs`, `fmt`, `disasm`, `build`, `info`), stateful notebook tools (`notebook/new`, `read`, `add_cell`, `update_cell`, `delete_cell`, `eval_cell`, `eval_all`, `export`) backed by a cached per-file evaluation engine, and user-defined tools via the `deftool` form (JSON-schema → positional args; visibility control via `:private`/`:mcp/expose` and `--include`/`--exclude`). Binaries produced by `sema build` accept `--mcp` to run as a standalone MCP server exposing their embedded tools. See PR #43 and the [MCP docs](https://sema-lang.com/docs/mcp.html).
- **DAP debugger ergonomics** — the debugger is now usable by default (works with `stopOnEntry: false`). Adds line-aware **verified breakpoints** that slide over blank/comment lines, `evaluate` and `setVariable` while paused (including write-through of a top-level `(set! …)` to in-scope locals/upvalues/globals), named upvalues under a *Closure* scope, named record-field expansion, lazy expansion of compound values, and pc-scoped locals so out-of-scope bindings are hidden. See PR #44 and the [DAP docs](https://sema-lang.com/docs/dap.html).

### Changed

- **Bytecode format bumped to version 4** — version 3 added per-function upvalue names; version 4 adds per-function local block-scope ranges, both used by the DAP variable inspector (so precompiled `.semac` shows correctly pc-scoped locals). Older `.semac` files are rejected with a clear "recompile from source" error; recompile with `sema compile`. See `website/docs/internals/bytecode-format.md`.
- **`.semac` bytecode is now stack-verified on load** — a sound abstract stack-depth verifier runs inside the deserializer and rejects unbalanced/underflowing bytecode (including crafted exception-handler depths) before execution, closing a memory-safety hole for untrusted/corrupt `.semac` files (audit finding C11 / ADR #56).
- **`sema-fmt` public API consolidated into a single entry point** (breaking for Rust consumers of the `sema-fmt` crate; `sema fmt` CLI flags, `sema.toml` config, LSP formatting, and the playground's `formatCode` JS API are all unchanged). `format_source(input, width)` and `format_source_opts(input, width, indent, align)` are replaced by `format_source(input: &str, opts: &FormatOptions)`. `FormatOptions { width, indent, align }` implements `Default` (width 80, indent 2, align off) and is now the single source of truth for formatter defaults — the CLI's `sema.toml` fallbacks and the LSP's formatting handler both derive from it instead of repeating the values.
- **`sema-fmt` internals cleaned up**: the test suite moved from an in-file `#[cfg(test)]` module to `crates/sema-fmt/tests/formatter_test.rs` (public-API integration tests), `formatter.rs` gained module- and method-level documentation describing the formatting pipeline and per-form layouts, and duplicated flat-rendering helpers were consolidated.

## 1.16.0

REPL + security-hardening release. The REPL foundation moved from rustyline to reedline (no user-facing flag changes, no script breakage), gaining syntax highlighting, bracket matching, ghost-text completion hints, an arrow-key value inspector, and the `,disasm` / `,apropos` commands. Alongside that, a whole-codebase security/correctness audit (`docs/bugs/2026-05-29-*`) was triaged and its P0 and P1 findings fixed — closing two denial-of-service / secret-leak P0s and a cluster of SSRF, path-traversal, UB, and editor-correctness P1s.

### Added

- **REPL syntax highlighting** with bracket matching as you type.
- **Ghost-text inline hints** suggesting completions from history and the global environment.
- **`,inspect <expr>`** opens an interactive arrow-key navigator for lists, vectors, and maps — drill into nested structures without re-typing accessors.
- **`,disasm <fn>`** prints the bytecode for a named function or top-level form.
- **`,apropos <pattern>`** searches global names by substring or regex.

### Changed

- **REPL foundation: rustyline → reedline.** No breaking changes to REPL commands or flags; the migration unlocks the highlighting/hinter/validator surface above. See PR #37.
- **VM "unbound global" errors carry a `Did you mean …?` hint** when the name is close to an existing binding (matches the tree-walker's behaviour).

### Fixed (security)

- **Gemini API key no longer sent in the URL query string** (P0). The key now travels in the `x-goog-api-key` header and the request-controlled `model` is validated before being interpolated into the path, closing both a key-leak-into-logs vector and an SSRF/path-injection vector.
- **Untrusted `.semac` can no longer crash the VM via `CallNative`** (P0). The native-table bounds check is now a real runtime check (was `debug_assert!`, compiled out in release).
- **Provider `base-url` SSRF blocked under the sandbox** (P1). When running untrusted/sandboxed code, `llm/configure` / `llm/configure-embeddings` reject base URLs pointing at loopback/private/link-local hosts (e.g. `169.254.169.254`); trusted CLI/REPL/notebook sessions keep full access so local proxies and Ollama still work. The check also decodes obfuscated `inet_aton` IP forms — decimal (`2130706433`), octal (`0177.0.0.1`), hex (`0x7f.0.0.1`), and short (`127.1`) — that resolve to internal addresses, so they can't slip past the gate.
- **Registry package-name path traversal blocked** (P1). `sema pkg add` now validates registry names before joining them into `~/.sema/packages/`, so a name like `../../etc/cron.d` can no longer escape the packages dir.

### Fixed (correctness & reliability)

- **`display` / `print` output is no longer erased in the interactive REPL.** Output without a trailing newline was wiped by the prompt repaint under the old rustyline REPL; the reedline migration fixes it, now guarded by a pty-driven regression test.
- **`DUP` on an empty operand stack returns a clean error instead of reading out of bounds** (P1, UB) — reachable only from crafted/corrupt bytecode.
- **Unbounded HTTP request bodies are capped at 16 MiB** (P1). `http/serve` returns `413` instead of buffering an arbitrarily large body into memory.
- **`ws/close` actually closes the socket** (P1). It now releases the sole outgoing sender (previously dropped a throwaway clone, so the connection stayed open until the handler returned).
- **Rate-limit retry no longer panics on a backward clock adjustment** (P1) — the elapsed-time subtraction now saturates.
- **The debugger no longer hangs on `stackTrace` while the program is running** (P1, DAP). State queries received mid-run are now answered instead of dropped, which previously leaked a blocking thread and froze the session.
- **LSP positions are now correct on lines containing emoji / astral characters** (P1). Sema spans count characters while LSP counts UTF-16 code units; the two are now converted in both directions, so diagnostics, go-to-definition, references, document highlights, and **rename** land on the right columns (rename could previously edit the wrong span on such lines).

### Security

- **Dependency bumps to clear 19 Dependabot advisories** (PR #38): `rand` 0.9.2 → 0.9.3 and 0.10.0 → 0.10.1, `rustls-webpki` 0.103.10 → 0.103.13 in the root workspace; `openssl` 0.10.75 → 0.10.80, `rand` 0.8.5 → 0.8.6, `rustls-webpki` 0.103.9 → 0.103.13 in `pkg/`; `pytest` 9.0.2 → 9.0.3 and `Pygments` 2.19.2 → 2.20.0 in the LSP e2e harness; `postcss` and `rollup` in the website.
- **VitePress 1.6.4 → 2.0.0-alpha.17** (PR #39) closes the vite `.map` path-traversal advisory (GHSA-4w7w-66w2-5vf9), which had no fix in the vite 5.x line vitepress 1 was pinned to.

### Docs

- The VM is now documented as the default backend across the website.
- Refreshed 1brc benchmark numbers.
- Added LSP audit harness design doc.
- Pruned shipped items from `docs/wip.md`.

## 1.15.0

Big quality-sweep release. ~120 audit findings triaged across six waves, of which ~60 shipped here. No new headline features — this release is about hardening, consistency, and error UX in the v1.14 async / sandbox / REPL surface.

### Fixed (correctness)

- **Top-level `(async ...)` side effects no longer vanish.** The CLI now drains pending tasks at exit. Previously a top-level spawned task whose promise wasn't awaited would be silently dropped along with its side effects.
- **`channel/close` reports the lost value** when called on a channel with a blocked sender. Previously: a generic "channel closed" error after the pending send was already lost.
- **`async/timeout` clamps absurd durations.** `(async/timeout 9999999999999 …)` now errors with a clear "exceeds maximum" message instead of waiting ~317 years.
- **Doubled error prefix on nested awaits removed.** `(await (async (await (async/rejected "boom"))))` now reports `async/await: task rejected: boom` instead of `Eval error: async/await: task rejected: Eval error: async/await: task rejected: boom`.
- **`http/file` is now sandbox-gated** (`FS_READ` + path-check). Previously it canonicalized arbitrary host paths and served them to network clients even under `--sandbox=strict --allowed-paths=./data`.
- **`db/exec` / `db/exec-batch` / `db/query` / `db/query-one` / `db/last-insert-id` / `db/tables` are now sandbox-gated** (`FS_WRITE` / `FS_READ`). SQL `ATTACH DATABASE` could previously bypass `--allowed-paths`.
- **`sys/home-dir` / `sys/user` / `sys/temp-dir` / `sys/cwd` now require `ENV_READ`.** They were leaking environment values while sibling `env` / `sys/env-all` were already gated — inconsistent surface.
- **`shell` now requires both `SHELL` and `PROCESS`.** Subprocess execution is properly classified as a process operation.
- **`string/repeat` validates count `>= 0`** before the `usize` cast (was a user-input panic on negative).
- **`abs i64::MIN` errors** via `checked_abs` (was returning `i64::MIN`).
- **`nth` / `take` / `drop` reject negative indices** with a clear error + hint (were silently casting i64 → huge usize, producing confusing errors or returning wrong results).
- **Notebook engine now honors a wall-clock per-cell deadline** (env `SEMA_NOTEBOOK_TIMEOUT_MS`, default 30 s). `(while #t)` and infinite recursion no longer brick the server.
- **Notebook `undo_last_cell` restores downstream stale flags** it marked. Previously the documented "downstream stale markers are reverted on undo" behavior was false; downstream cells stayed stale forever after undo.
- **VM closures called from stdlib HOFs can now yield** (fix shipped in 1.14.3, listed here for completeness with the rest of the audit work). See 1.14.3 entry for details.

### Changed (async semantics pass — A1 + A4 + D2)

- **`(async/cancel p)` returns a boolean** — `#t` if the call actually transitioned the promise into `Cancelled`, `#f` if there was nothing to cancel (already resolved / rejected / cancelled, or never spawned via `async/resolved` / `async/rejected`). Previously: returned `nil` on success and **errored** with `"async/cancel: cannot cancel a non-spawned promise"` for never-spawned promises. Cancellation is now strictly best-effort and never raises.
- **Cancellation is now a peer `PromiseState::Cancelled` variant** instead of `Rejected("cancelled")`. `(async/cancelled? p)` matches the variant directly — a user `(async/rejected "cancelled")` no longer fools the predicate. `(async/rejected? p)` returns `#f` for cancelled promises (the four state predicates now cleanly partition the terminal states).
- **Awaiting a cancelled promise** raises `"async/await: task was cancelled"` (with a hint) instead of surfacing as `"task rejected: cancelled"`. `async/all` and `async/timeout` also distinguish cancellation from rejection in their error messages.
- **Scheduler ready-task pickup is now strictly FIFO.** Previously `swap_remove` rearranged the queue and produced a LIFO-feeling surface under contention: three sequential channel sends followed by three sequential receives returned `(1 3 2)` instead of `(1 2 3)`. The fix uses `Vec::remove` (O(n) per pickup, negligible for typical task counts).
- **`(force x)` on a non-thunk now errors** instead of silently passing the value through.

### Changed (canonical naming, Decision #59)

Legacy names stay registered as aliases; new code should prefer the canonical form.

| Legacy | Canonical |
|---|---|
| `any` | `any?` |
| `every` | `every?` |
| `time-ms` | `time/now-ms` |
| `hash-map` | `map/new` |
| `promise-forced?` | `async/forced?` |
| `tools->routes` | `route/from-tools` |
| `make-bytevector` | `bytevector/make` |
| `bytevector-{length,u8-ref,u8-set!,copy,append,->list}` | `bytevector/{length,u8-ref,u8-set!,copy,append,to-list}` |
| `list->bytevector` | `bytevector/from-list` |

**Path operations consolidated.** `path/dirname` / `path/dir`, `path/basename` / `path/filename`, `path/ext` / `path/extension` are no longer independent registrations with divergent edge-case behavior — they share a single implementation. Behavior is now uniform: no-parent / no-extension returns `""` (was `nil` for the legacy names). Canonical names are `path/dir`, `path/filename`, `path/extension`.

### Changed (error UX)

- 52 stdlib functions now attach a `.with_hint(…)` to their type errors pointing at the expected argument shape (`get`, `assoc`, `dissoc`, `map`, `filter`, `foldl`, `for-each`, `sort-by`, `partition`, `nth`, `take`, `drop`, `json/decode`, arithmetic, etc.).
- `cond` clause errors now use the `cond:` prefix matching the rest of the special-form family (`case:`, `match:`, `do:`, `let:`).
- `/`, `mod` divide-by-zero errors carry function prefix + hint (`"/: guard with (if (zero? d) ... (/ n d))"`).
- `json/decode`, `toml/decode`, `csv/parse`, `time/parse` parse errors now include line/column where available + actionable hints.
- `string-ref` out-of-bounds errors include the string length and a 0-based-indexing hint.

### Changed (CLI / REPL)

- **`sema notebook run` prints captured stdout** (was swallowing it for headless runs).
- **`sema` file-not-found wording unified** across `--load`, `<file>`, `compile`, `build`, `fmt`, `ast`. Now: `error: file not found: <path>` (was three different shapes, some leaking `(os error 2)`).
- **`sema notebook` (no subcommand) prints a proper error line** before help. Previously: exited 2 with help text and no `error:` line.
- **`sema build` pre-flights `--output` writability.** Previously it ran four of five steps before failing on permission denied.
- **`sema -e EXPR <FILE>` is now an explicit clap conflict.** Previously the positional file was silently dropped.
- **`--vm` is a hidden no-op alias** so muscle memory from older docs doesn't get "unexpected argument" (VM is the default since v1.14).
- **REPL silent-define fixed.** `(define x …)` now prints `; defined x` (dim) instead of nothing.
- **REPL EOF on unterminated input** now reports `error: unterminated input at EOF` and exits 1 (was: silently dropped the buffered form).
- **REPL `,env`** snapshots prelude keys at start and filters them out — shows only user-added bindings.
- **REPL `,doc` reports VM closures correctly** (`square : lambda (arg0)` instead of `native-fn`) and falls back to builtin docstrings for natives and special forms.
- **REPL accepts bare `quit` / `exit` / `:q`** alongside `,quit` / `,exit` / `,q`.

### Changed (LSP)

- **`workspace/symbols` now iterates the workspace scanner's `import_cache`** in addition to open documents. Previously the workspace scanner ran but its results were invisible to Ctrl+T.
- **Code lens / executeCommand handlers use the parse cache.** Previously they re-parsed the document on every keystroke-driven request.
- **Folding ranges require `>= 2` visible lines** — cuts the per-paren noise that was clogging the gutter for any nested list.
- LSP shutdown 2-second force-exit now carries a `TODO(tower-lsp#399)` comment with the upstream link.

### Changed (docs & website)

- Stdlib doc pages corrected for ~12 documented inaccuracies: `(/ 10 3)` returns float not int; `list/index-of` returns `nil` not `-1`; `print` is not an alias for `display`; the `foldl` consumer example in concurrency.md actually works; `path/extension` of `"Makefile"` returns `""`; `(type X)` returns keyword not string; `while` is now documented; `1e10` was removed (reader doesn't accept scientific notation); etc.
- Sandbox capability table in `cli.md` regenerated from actual `register_fn_gated` / `register_fn_path_gated` call sites (was missing ~15 gated functions across `fs-read`, `fs-write`, `network`, `process`, and the `serial` capability for `--sandbox=strict`).
- `sema fmt --json` flag documented; the bogus `sema notebook export -f` short flag removed.
- New "Scheduling guarantees" section in `concurrency.md` documenting FIFO pickup + wake order + cooperation.
- New `website/docs/stdlib/serial.md` covering the `serial/*` functions (added in v1.13 but undocumented).
- Notebook docs got a "REST API" section + VFS scope warning.

### Added

- `PromiseState::Cancelled` variant in `sema-core`.
- `EvalContext::eval_deadline` (Cell<Option<Instant>>) honored by both the trampoline and the VM dispatch loop. The notebook engine uses it via `SEMA_NOTEBOOK_TIMEOUT_MS`.
- New dual-eval / vm-async tests across the audit (~30 net new tests pinning post-fix behavior, including 8 for the async semantics pass alone, 5 for the input-validation cluster, regression tests for T2 undo, etc.).
- New playground examples in a `Concurrency` category (`channels.sema`, `parallel-tasks.sema`, `timeout.sema`).
- 12 `// SAFETY:` blocks added to previously-unjustified `unsafe { … }` blocks in the VM and `Value::view` heap-tag dispatch.

### Internal

- `get_sequence` in stdlib HOFs returns `&[Value]` instead of allocating a fresh `Vec<Value>` per call. No measurable perf change on the HOF benchmark — kept as a code-quality improvement.
- `dual_eval_error_tests!` macro extended with a `name : input => "expected_substr"` form. 58 of 59 existing call sites migrated to assert specific error substrings instead of just `.is_err()`.
- `MAKE_LIST` / `MAKE_VECTOR` opcodes use `Vec::split_off` instead of `drain(start..).collect()`.

### Known Limitations

- **VM `set!` through stdlib HOF callbacks is silently lost** (audit finding C1). `(let ((c 0)) (map (fn (x) (set! c (+ c x))) (list 1 2 3)) c)` returns `0` on the VM and `6` on the tree-walker. Root cause is the eager-close + dual-write upvalue model; the planned fix is an open-upvalue runtime (see `docs/adr.md` #55 and `docs/limitations.md` #31). Workaround: use `--tw`, or thread state via `foldl` instead of captured `set!`. Related symptoms: `(type (fn (x) x))` is `:native-fn` on VM vs `:lambda` on TW; VM caught-error maps are missing `:stack-trace`.
- **`.semac` bytecode loading is unsafe from untrusted sources** (audit finding C11). `validate_bytecode` does not abstract-interpret the instruction stream for stack balance; the VM's `pop_unchecked` (90+ call sites) assumes stack-balanced bytecode, so a hand-crafted `.semac` with a leading `Pop` triggers UB in release builds. Treat `.semac` files as trusted-source-only until the stack-depth verifier in `docs/adr.md` #56 lands.

## 1.14.3

### Fixed

- **Stdlib higher-order functions can now yield** — `for-each`, `map`, `filter`, `foldl`, `foldr`, `reduce`, `sort-by`, `partition`, `any`, `every`, `apply`, etc. now correctly suspend when their lambda callback performs an async operation (`channel/send` on a full buffer, `channel/recv` on an empty one, `await`, `async/sleep`). Previously, the inner closure's `AsyncYield` was translated to `"async yield outside of scheduler context"` and surfaced as a deadlock in the owning task, because VM closures called from outside the VM hit a fallback path that ran them on a fresh VM with `vm.run` (which can't yield). The fallback now routes through the scheduler when in async context, registering the closure as a real task and re-entering the (already re-entrant) scheduler until it completes.
- **Yielding native passed directly to a HOF now errors clearly** — patterns like `(map channel/recv (list ch ch ch))` previously silently coalesced yields across iterations and produced wrong results (sometimes returning a single value instead of a list). They now raise an explicit error pointing to the lambda-wrap workaround: `(map (fn (c) (channel/recv c)) ...)`.

### Added

- `website/docs/stdlib/concurrency.md` — new "Async ops inside higher-order functions" section explaining the lambda-wrap idiom.

## 1.14.2

### Fixed

- **npm publish workflow** — switched the GitHub Actions runner to Node 24.x so npm has a working `arborist`. Node 22.22.2's bundled npm was missing `promise-retry`, causing every `npm install` / `npm publish` to fail since v1.13.0. Identical source to v1.14.1; this release exists to ship the four missing npm versions (`1.12.3`, `1.13.0`, `1.14.0`, `1.14.1`, `1.14.2`) by getting the workflow green again.

## 1.14.1

### Fixed

- **Linux release builds** — added `libudev-dev` to the cargo-dist apt dependency list so the `serialport` crate (transitive `libudev-sys`) compiles on the GitHub Actions Linux runners. Without this, the `Release` workflow's `build-local-artifacts (x86_64-unknown-linux-gnu)` and `(aarch64-unknown-linux-gnu)` jobs failed during 1.14.0, blocking the GitHub release page, Homebrew tap update, and binary uploads. 1.14.0 was published successfully to crates.io but had no downloadable binaries; 1.14.1 is the same code with a working binary release.

## 1.14.0

### Added

- **VM async concurrency** — cooperative VM-per-task scheduler with promises and channels. Each `(async/spawn fn)` creates a dedicated VM sharing globals/functions with the parent; round-robin scheduling, no replay, side effects execute exactly once. Async features are **VM-only** (tree-walker returns a clear error).
  - **Special forms**: `(async body...)`, `(await promise)`
  - **Promise stdlib**: `async/spawn`, `async/await`, `async/all`, `async/race`, `async/sleep` (real timing), `async/timeout`, `async/resolved`, `async/rejected`, plus predicates
  - **Channel stdlib**: `channel/new`, `channel/send`, `channel/recv`, `channel/try-recv`, `channel/close`, `channel/closed?`, `channel/count`, `channel/empty?`, `channel/full?`
  - **Cancellation**: `async/timeout` enforces real deadlines; tasks cancel cleanly
  - **Yield mechanism**: thread-local signal checked after every native call; on yield the VM leaves a placeholder on the stack and resumes when the scheduler wakes the task
  - **Re-entrant scheduler**: nested `async/spawn` and `async/await` inside tasks work correctly
  - **New value types**: `AsyncPromise` (tag 28), `Channel` (tag 29) with full NaN-box support
  - **Docs**: new `/docs/stdlib/concurrency.html` reference page; `async`/`await` in special-forms docs
- **Interactive CLI primitives** — terminal UI building blocks (Unix-only, no-op stubs elsewhere):
  - **EOF detection**: `io/read-line` now returns `nil` on EOF (was `""`); new `io/eof?` predicate; `io/flush` for explicit stdout flush
  - **Raw-mode TTY**: `io/tty-raw!` / `io/tty-restore! token` via `cfmakeraw`/`tcsetattr`
  - **Keystroke reader**: `io/read-key` and `io/read-key-timeout ms` returning a map (`:char` / `:ctrl` / `:key` / `:alt`); handles CSI/SS3 escape sequences (arrow keys, F-keys, Page Up/Down, Delete), UTF-8 multi-byte chars with continuation timeout, and control characters
  - **Terminal size**: `sys/term-size` → `{:rows N :cols M}` via `ioctl(TIOCGWINSZ)`
  - **Signal hooks**: `sys/on-signal :winch|:int|:term callback` + `sys/check-signals` (async-signal-safe handlers, callbacks invoked from event loop)

### Changed

- **`io/read-line` returns `nil` on EOF** instead of `""` — programs can now distinguish closed stdin from empty input. Use `io/eof?` for non-breaking checks.

## 1.13.0

### Added
- **Notebook interface** (`sema notebook`) — Jupyter-inspired cell-based notebook with browser UI, stdout capture, single-cell undo with environment rollback, Alpine.js templates, `.sema-nb` JSON format, Sema branding (black/gold palette)
- **SQLite integration** — `db/open`, `db/open-memory`, `db/exec`, `db/exec-batch`, `db/query`, `db/query-one`, `db/last-insert-id`, `db/tables`, `db/close` via bundled rusqlite; parameterized queries, WAL mode, foreign keys enabled by default
- **Typed numeric arrays** — `f64-array` and `i64-array` types with dedicated VM intrinsics (15,000x faster than list foldl for numeric workloads)
- **VM intrinsics** — `Mod` and `Nth` opcodes for faster modulo and indexed access
- **Loop macros** — `dotimes` and `for-range` counted iteration macros
- **Meta package registry** — GitHub-linked packages with upstream redirect, SeaORM migration, multi-database support (SQLite/PostgreSQL/MySQL)
- **Registry admin panel** — Dashboard with user/package management, audit logging, moderation queue, ban enforcement
- **Download tracking** — Daily download aggregation with sparkline visualization
- **Package README display** — Server-side GitHub-flavored markdown rendering with syntax highlighting
- **Package reporting** — "Report this package" feature with moderation queue
- **Demo notebook** (`examples/notebook/demo.sema-nb`) — 16-cell Sema language tour
- **Notebook E2E tests** — 41 Playwright browser tests
- **Registry E2E tests** — 63 Playwright browser tests + 69 Rust integration tests
- **`time/ms`** — Millisecond timestamp stdlib function

### Changed
- `make examples` now skips `eliza.sema` (interactive) and `eliza-web.sema` (server) to prevent blocking during smoke tests
- Package registry migrated from raw sqlx queries to SeaORM entity models (13 entities)

### Fixed
- Notebook stdout capture — `println`/`display`/`print` output now appears in cell output instead of server terminal

## 1.12.3

### Added

- **Custom validation predicates for `llm/extract`** — field specs now support `:validate` with a predicate function that runs after type checking. Failed predicates trigger the existing retry/re-ask loop with actionable error context. Example: `{:amount {:type :number :validate #(> % 0)}}`.
- **Optional fields in `llm/extract` schemas** — field specs with `:optional #t` no longer trigger "missing key" errors when absent from the LLM response. Example: `{:bio {:type :string :optional #t}}`.
- **Custom validation error messages** — field specs support `:message` for human-readable error text used in re-ask prompts. Example: `{:age {:type :number :validate #(>= % 0) :message "age must be non-negative"}}`.

## 1.12.2

### Fixed

- **LLM functions now accept both lists and vectors** — `llm/chat`, `llm/classify`, `llm/batch`, `llm/embed`, `llm/pmap`, `llm/similarity`, `llm/token-count`, `llm/with-fallback`, and tool parameter extraction all now accept vectors `[...]` in addition to lists `(list ...)`. Previously, passing a vector (e.g., `[(message :user "hi")]`) to `llm/chat` would fail with `"expected list of messages or prompt, got vector"`. Added `Value::as_seq()` helper that matches both list and vector types.
- **Docs and examples updated** — all LLM examples in the website, README, and example files now use canonical `(list ...)` syntax.

## 1.12.1

### Added

- **ELIZA chatbot example** — `examples/eliza.sema`, a faithful reimplementation of Weizenbaum's 1966 chatbot with keyword priorities, wildcard pattern matching, pronoun reflection, and response cycling.
- **ELIZA web interface** — `examples/eliza-web.sema`, a self-contained retro CRT-styled web UI for the ELIZA chatbot using `http/serve` with `POST /chat` endpoint.
- **HTTP server test suite** — ~110 new tests across dual-eval, unit, and integration tiers:
  - 70 dual-eval tests for response helpers (`http/ok`, `http/created`, `http/redirect`, `http/html`, `http/text`, etc.)
  - 27 router unit tests (matching, params, wildcards, trailing slashes, method dispatch, handler errors)
  - WebSocket integration tests (multi-message echo, server-initiated close)
  - SSE streaming tests (multiple events, content-type verification)
  - Error resilience tests (handler panic recovery, concurrent requests, large bodies, custom headers)
  - Middleware pattern tests (CORS wrapper, logging wrapper)
  - Construction error tests for `http/router`, `http/stream`, `http/websocket`
- **Correctness test suite** — ~140 new tests across the core language:
  - 78 datetime dual-eval tests (leap year, midnight rollover, epoch, century leap, error cases)
  - 27 shadowed-builtin tests (all foldable operators across `let`, `let*`, `letrec`, `lambda`, `define`, `do`)
  - Float edge-case tests (`-0.0` equality, `eq?`, `equal?`, `zero?`, collection membership)
  - Unicode string tests (`string/index-of`, `string/last-index-of` with multi-byte characters)
  - String error tests (negative/OOB indices for `string-ref` and `substring`)
- **LSP refactor** — split helpers and scope analysis out of monolithic `lib.rs` into `helpers.rs` and `scope.rs`; added special form documentation for hover.
- **Formatter** — added `--json` output mode with NDJSON multi-file support and `file` field.
- **VM** — optimized debug hook hot path; replaced `unsafe transmute` in `Op::from_u8()` with safe match + compile-time exhaustiveness check.
- **VM** — removed dead `NamedLet` variant, `resolve_named_let`, and `compile_named_let` (desugared to `letrec+lambda` in lowering since Decision #52; code was unreachable).
- **VM** — unified `CoreExpr` and `ResolvedExpr` into a single generic `Expr<V>` type, halving the surface area for new language constructs.
- **VM** — per-instruction inline cache for `LoadGlobal`/`CallGlobal`, replacing the 256-slot direct-mapped hash cache. Each instruction gets a dedicated cache slot, eliminating hash collisions. Bytecode format bumped to v2.
- **VM** — Lua-style open upvalues: upvalue cells hold a stack index instead of an eagerly-copied value, closed at frame exit (Return, TailCall, exception unwind) and before non-VM calls. Eliminates the `has_open_upvalues` branch and dual-write pattern from 10 LoadLocal/StoreLocal opcodes.
- **VM** — `compile_program_with_spans` now returns `CompiledProgram` struct (was a tuple).
- **file/read-lines** — switched from `split('\n')` to `.lines()`, correctly handling `\r\n` line endings. Empty files now return an empty list instead of a single-element list containing `""`.
- **kv/open** — malformed JSON in an existing backing file now raises an IO error instead of silently falling back to an empty store.

### Fixed

- **VM** — optimizer now correctly skips constant folding when builtins (`+`, `-`, `*`, `/`, `<`, `>`, `=`, `not`, etc.) are shadowed by local bindings. Previously `(let ((+ *)) (+ 3 4))` would be miscompiled to `7` instead of `12`. The fix tracks shadowed names through all binding forms (`let`, `let*`, `letrec`, `lambda`, `do`, `try/catch`) during optimization.
- **VM** — compiler no longer emits intrinsic opcodes (`Op::AddInt`, etc.) for builtins that are redefined via top-level `define`. Previously `(begin (define + *) (+ 3 4))` would ignore the redefinition and use the hardwired addition opcode.
- **VM** — `Op::Negate` and `fold_unary_op` now use `wrapping_neg()` instead of bare `-n`, matching the stdlib's behavior and avoiding panics in debug builds on `i64::MIN`.
- **VM** — `vm_div` and optimizer division now use exact integer arithmetic (`x % y == 0` → `x / y`) instead of casting through `f64`, avoiding precision loss for integers above 2^53.
- **string/index-of**, **string/last-index-of** — fixed byte-offset vs character-offset bug; now correctly returns character indices for multi-byte strings (e.g., `(string/index-of "café world" "world")` now returns `5`, not `7`).
- **string-ref** / **substring** — negative indices now raise clear error messages (e.g., "index -1 must be non-negative") instead of silently wrapping via `as usize` and giving misleading "out of bounds" errors.
- **Equality** — `Value::PartialEq` for floats now uses IEEE 754 equality (`a == b`) instead of bit comparison. `-0.0` and `0.0` compare as equal via `equal?` and `eq?`, and `Value::Hash` normalizes `-0.0` so equal values hash identically.
- **Float ordering** — `Value::Ord` now uses `f64::total_cmp()` instead of `to_bits().cmp()`, fixing incorrect ordering of negative floats (previously all negatives sorted after all positives).
- **split_identifier_words** — uses `chars().count()` instead of byte `len()` for word boundary detection, fixing potential incorrect splits with multi-byte uppercase characters.
- **http/stream**, **http/websocket** — now validate that the argument is a function at construction time. Previously `(http/stream 42)` silently accepted non-functions, only failing later when the server tried to call them.
- **Docs** — `string/index-of` docs corrected from "byte index" to "character index"; `string/last-index-of` docs corrected from "-1" to "nil" for not-found return value.
- **CI** — fixed lychee link checker config, broken doc link, and git test identity issues.
- **VM** — inner define forward references now work correctly (R5RS `letrec*` semantics). Functions like `(define (a) (b)) (define (b) 42)` inside a function body can forward-reference each other. This fixes the nqueens benchmark which was broken on the VM.
- **VM** — fixed stale upvalue cell reuse when local slots are reused across named-let scopes with intervening native calls. `close_open_upvalues` now clears entries after closing, preventing `make_closure` from reusing Closed cells containing old variable values.
- **I/O** — `display` and `print` now flush stdout immediately, fixing missing prompts before `read-line` in interactive programs.
- **Test suite hardening** — comprehensive audit and fix of fragile tests across the entire codebase:
  - Migrated 20 tests from hardcoded `/tmp` paths to unique temp directories
  - Added read timeouts and `wait_for_event()` helper to DAP integration tests
  - Combined racing VFS `OnceLock` tests into single sequential test
  - Replaced exact float equality with epsilon comparisons for transcendental functions
  - Added structural `matches!` assertions on error variants alongside display checks
  - Fixed deftool tests to verify tool metadata instead of unrelated lambda calls
  - Used `CARGO_MANIFEST_DIR` for CWD-independent glob tests
  - Widened timezone-sensitive timestamp ranges
  - Added `#[cfg(unix)]` guards on Unix-specific tests (shell, which, path separators)
  - Replaced eval-tw oracle with hand-constructed `Value::list()` in foundational dual-eval tests
  - Wrapped order-dependent collection tests in `sort` for BTreeMap independence
  - Documented 10 test fragility categories in `docs/bugs/`
- **Test error assertions** — replaced ~40 fragile `.contains("expects")`/`.contains("Type error")` string assertions with structural `SemaError::Arity`/`SemaError::Type` variant matching via new `assert_arity_error()`/`assert_type_error()` helpers.

## 1.12.0

### Added

- **Language Server Protocol (LSP)** — new `sema-lsp` crate (`crates/sema-lsp/`) with `sema lsp` subcommand providing a full-featured LSP server for Sema:
  - **Parse diagnostics** — real-time error squiggles for syntax errors with error recovery (reports multiple errors). Uses `read_many_with_spans_recover` for resilient parsing.
  - **Compile-time diagnostics** — unbound variable, arity mismatch, and invalid form detection via `sema_vm::compile_program`, shown as warnings.
  - **Completion** — special forms, stdlib builtins, user-defined symbols, and scope-aware local bindings (let/lambda params). Local bindings sort before globals.
  - **Go to definition** — user-defined symbols (same file), local bindings (lambda params, let variables), cross-file module definitions via import/load path resolution, and import path strings → file navigation.
  - **Hover documentation** — 160+ builtin docs scraped from sema-lang.com, user function signatures with params, special form labels, and imported symbol provenance.
  - **Find all references** — scope-aware: local variables only show same-scope references; top-level symbols search across all open documents, excluding shadowed occurrences.
  - **Rename** — scope-aware: renaming a local binding only renames within its scope; renaming a top-level symbol renames across all open files, skipping shadowed locals. Blocks renaming builtins and special forms.
  - **Document symbols** — outline view of all top-level definitions (defun, defn, define, defmacro, defagent, deftool) with symbol kinds and precise name ranges.
  - **Workspace symbols** — search user definitions across all open documents.
  - **Signature help** — parameter highlighting for user-defined and imported functions, with builtin doc fallback.
  - **CodeLens** — "▶ Run" lens on every top-level form, executing via `sema eval` subprocess with inline result display via `sema/evalResult` custom notification.
  - **Scope tree** (`scope.rs`) — lexical scope analysis built from the AST, tracking all binding forms: `define`, `defun`/`defn`, `defmacro`, `lambda`/`fn`, `let`/`let*`/`letrec`, named `let`, `match`, `do`, `try`/`catch`, `for` variants. Powers scope-aware rename, references, go-to-definition, and completion.
  - **Threading model** — dedicated backend thread owns all `Rc` state; async LSP handlers communicate via channel + oneshot.
  - **Caching** — per-document `CachedParse` (AST, SpanMap, symbol spans, scope tree) updated on every edit; per-import-file cache with mtime invalidation. All request handlers use cached data (no redundant re-parsing).

- **Debug Adapter Protocol (DAP)** — new `sema-dap` crate (`crates/sema-dap/`) with `sema dap` subcommand for step-debugging Sema programs:
  - **VM debug hooks** — `DebugState` with breakpoint management, step modes (StepIn, StepOver, StepOut, Continue), and per-instruction debug callback in the VM run loop.
  - **Source span propagation** — compilation pipeline now tracks source file and spans through lowering → optimization → resolution → compilation, enabling source-level debugging.
  - **Debug inspection** — stack, locals, and upvalue inspection methods on the VM for variable display during paused execution.
  - **DAP server** — stdio-based JSON transport implementing initialize, launch, setBreakpoints, continue, next, stepIn, stepOut, threads, stackTrace, scopes, variables, evaluate, and disconnect.
  - **VS Code integration** — launch configuration example for VS Code DAP client.

- **IntelliJ IDEA plugin** (`editors/intellij/`) — full IDE support via LSP4IJ:
  - Syntax highlighting (keywords, strings, numbers, comments, symbols, keywords).
  - Custom lexer matching Sema's syntax (`:keywords`, `#"regex"`, `f"strings"`, `#| block comments |#`).
  - Brace matching and commenting (line `;` and block `#| |#`).
  - Run configurations for `.sema` files.
  - File type registration for `.sema` and `.semac` with custom icons.
  - LSP client with `sema/evalResult` notification handling and inline result display.
  - Color settings page with configurable syntax colors.

### Changed

- **`Span` derives `PartialEq, Eq`** — enables scope tree reference comparison without manual field matching.

### Documentation

- **DAP design doc** — `docs/plans/2026-02-25-dap-debug-adapter.md` and implementation plan.
- **IntelliJ plugin plan** — `docs/plans/2026-02-25-intellij-plugin.md` and eval result notification spec.
- **LSP docs updated** — `website/docs/lsp.md` updated with rename, references, document symbols, signature help.
- **CLI docs updated** — `website/docs/cli.md` updated with `sema lsp` and `sema dap` subcommands.
- **Editors docs updated** — `website/docs/editors.md` updated with IntelliJ plugin section.

### Internal

- **`sema-vm` debug module** — new `debug.rs` with `DebugState`, `StepMode`, breakpoint tracking, and `source_file` on `Function`.
- **`sema-vm` span tracking** — `CoreExpr` nodes carry source spans through the compilation pipeline; `Chunk.spans` populated during normal compilation.
- **LSP builtin docs** — `builtin_docs.rs` generates markdown documentation for 160+ stdlib functions from curated data.
- **111 LSP tests** — comprehensive test suite covering diagnostics, completion, symbol extraction, scope tree resolution, rename scoping, and reference scoping.

## 1.11.0

### Added

- **Auto-gensym (`foo#`)** — Clojure-style automatic gensym in quasiquote templates. Symbols ending with `#` inside backtick forms are replaced with unique generated symbols, preventing variable capture in macros. Same `foo#` within one quasiquote maps to the same gensym; each quasiquote evaluation gets fresh symbols. Works in both tree-walker and bytecode VM. Built-in `some->` macro updated to use `v#` instead of hardcoded `__v`. Manual `(gensym)` and auto-gensym share a single counter to prevent collisions.
- **Code formatter (`sema fmt`)** — built-in formatter with Lisp-aware indentation for body forms, binding forms, clause forms, threading macros, and conditionals. Supports `--check`, `--diff`, `--width`, `--indent`, and `--align` flags. Preserves all comments, shebang lines, and multi-line strings. Idempotent output.
- **Project configuration (`sema.toml`)** — project-level config file with `[fmt]` section for `width`, `indent`, and `align` settings. Discovery walks up from CWD. CLI flags override config values.
- **Decorative alignment** — opt-in column alignment (`--align`) for consecutive defines, cond/case/match clauses, and let bindings.
- **Playground formatter** — "Fmt" button in the playground toolbar, powered by the formatter compiled to WASM.

- **Package manager (`sema pkg`)** — full CLI subcommand for managing Git-based packages: `sema pkg add` (install + auto-add to `sema.toml`), `sema pkg remove` (uninstall + auto-remove from `sema.toml`), `sema pkg install` (restore all deps from manifest), `sema pkg update`, `sema pkg list`, `sema pkg init`. Packages resolve from `~/.sema/packages/` with `package.sema` entrypoint convention.
- **Lock file (`sema.lock`)** — reproducible builds via `sema.lock`. Records exact commit SHAs for git packages and SHA256 checksums for registry packages. `sema pkg install` reads existing lock entries and resolves missing ones. `sema pkg install --locked` fails if lock is missing or out of sync with `sema.toml` (for CI). `sema pkg add`, `update`, and `remove` automatically maintain lock entries. Strict lock parsing with actionable error messages. Version/ref mismatch detection between `sema.toml` and `sema.lock`.
- **Package imports** — `(import "pkg-name")` now resolves packages from the local package store. Supported in both tree-walker and `sema build` import tracer. VFS-first resolution for bundled executables.
- **`toml/decode` and `toml/encode`** — new stdlib module for TOML parsing and serialization.
- **Prompt/conversation APIs** — 12 new LLM builtins: `prompt/append` (variadic), `prompt/concat`, `prompt/fill`, `prompt/slots`, `conversation/system`, `conversation/set-system`, `conversation/filter`, `conversation/map`, `conversation/say-as`, `conversation/token-count`, `conversation/cost`.
- **Static file serving** — `http/file` function and `:static` route type in `http/router` for serving static files with automatic MIME type detection, path traversal protection, directory `index.html` resolution, and SPA fallback.
- **Destructuring bind** — `let`, `define`, and lambda parameters now support destructuring lists (`(let (((a b) (list 1 2))) a)`), vectors, maps (`:keys` shorthand), and nested patterns. Works in both tree-walker and VM.
- **Pattern matching (`match`)** — `(match expr (pattern body) ...)` with literal, list, vector, map, `when` guards (`(pattern when guard body)`), rest (`&`), and wildcard (`_`) patterns. Dual-eval tested.
- **Multimethods (`defmulti`/`defmethod`)** — dispatch on return value of a discriminator function. Supports `:default` fallback method.
- **Regex literals** — `#"pattern"` raw string syntax for regex patterns. No escape processing except `\"`. Compiled to regex values at runtime via `regex/match`, `regex/find-all`, etc.
- **F-strings** — `f"Hello ${name}, you are ${(+ age 1)} years old"` with embedded `${expr}` interpolation.
- **Threading macros** — `->` (thread-first), `->>` (thread-last), `as->` (thread-as), `some->` (nil-short-circuiting) for pipeline-style data transformation.
- **Short lambdas** — `#(+ %1 %2)` Clojure-style anonymous function shorthand with `%` (alias for `%1`) and `%1`–`%9` positional parameters.
- **Nested map operations** — `get-in`, `assoc-in`, `update-in` for deep map access and modification.
- **Web server** — `http/serve` with Axum-based routing, path/query params, SSE streaming, and WebSocket support. Response helpers: `http/ok`, `http/created`, `http/no-content`, `http/not-found`, `http/html`, `http/text`, `http/redirect`, `http/error`, `http/stream`, `http/websocket`. Routing via `http/router`.
- **`sema build`** — compile Sema programs into standalone executables. Traces imports recursively, bundles source into a VFS archive appended to the binary. Auto-detected on load.
- **Cross-compilation (`sema build --target`)** — build standalone executables for other platforms. Downloads and caches pre-built runtime binaries from GitHub Releases. Supports `--target linux`, `--target macos`, `--target windows`, full triples, and `--target all`. Use `--no-cache` to force re-download, `--runtime` for custom binaries, or `SEMA_RUNTIME_BASE_URL` for self-hosted runtimes. Actionable error hints for download failures, format mismatches, and common mistakes.
- **`string/intern`** — opt-in string value interning with thread-local intern table. Returns shared `Rc<String>` for O(1) equality via NaN-boxed pointer comparison.
- **`while` special form** — `(while condition body...)` loop with imperative mutation. Returns the last body value or `nil`. Works in both tree-walker and VM.
- **Module/function aliases for legacy Scheme names** — 43 new slash-namespaced aliases: `string/length`, `string/append`, `string/ref`, `string/slice` (for `substring`), `string/to-symbol`, `symbol/to-string`, `string/to-keyword`, `keyword/to-string`, `number/to-string`, `string/to-number`, `string/to-float`, `char/to-integer`, `integer/to-char`, `char/to-string`, `string/to-char`, `string/to-list`, `char/alphabetic?`, `char/numeric?`, `char/whitespace?`, `char/upper-case?`, `char/lower-case?`, `char/upcase`, `char/downcase`, `map/new`, `map/deep-merge`, `map/get-in`, `map/assoc-in`, `map/update-in`, `bytevector/new`, `bytevector/length`, `bytevector/ref`, `bytevector/set!`, `bytevector/copy`, `bytevector/append`, `bytevector/to-list`, `list/to-bytevector`, `string/to-utf8`, `utf8/to-string`, `io/read-line`, `io/read-many`, `io/read-stdin`, `io/print-error`, `io/println-error`. Legacy names remain as silent aliases. Generic/polymorphic functions (`map`, `filter`, `foldl`, `length`, `append`, etc.) intentionally left un-namespaced.
- **Shebang support** — `#!/usr/bin/env sema` lines are ignored in source files.
- **`sys/sema-home`** — builtin returning the Sema home directory path.
- **"Did you mean?" suggestions** — unbound variable errors now suggest similar names using Levenshtein distance (threshold: 1/3 name length, max 3 suggestions). Searches both the current environment and a curated map of ~35 veteran hints from other Lisp dialects.
- **Veteran hints** — typing names from other dialects (e.g., `setq`, `funcall`, `loop`, `while`, `call/cc`) triggers targeted advice pointing to the Sema equivalent, before falling back to fuzzy matching.
- **Silent aliases for special forms** — `defn` (defun), `progn` (begin) are now accepted as silent aliases to support muscle memory from Clojure and Common Lisp.
- **Silent aliases for stdlib functions** — `mapcar` (map), `fold` (foldl), `some?`/`any?` (any), `every?` (every), `string-join`/`string-split`/`string-trim` (string/join, string/split, string/trim), `make-string` (string/repeat), `string-upcase`/`string-downcase` (string/upper, string/lower), `hash-map?` (map?), `hash-ref` (get), `type-of` (type).
- **REPL `,type` command** — evaluates an expression and displays its type (uses record tags for records).
- **REPL `,time` command** — evaluates an expression, prints the result, and displays elapsed execution time.
- **REPL `,doc` command** — displays binding information: whether it is a `native-fn`, `special form`, or `lambda` (including parameter lists).
- **REPL shadowing warnings** — `define` and `set!` now warn when shadowing a built-in native function.
- **REPL history search** — Ctrl-R reverse search through REPL history.
- **Prelude macros** — `when-let`, `if-let` for conditional binding.
- **Debug helpers** — `type` (value type as keyword), `spy` (labeled debug print to stderr), `time` (measure thunk execution time).
- **`sema completions --install`** — auto-installs shell completions to the standard location for Zsh, Bash, Fish, and Elvish. One command instead of manual mkdir + redirect.
- **`sema pkg init` scaffolds `package.sema`** — creates both `sema.toml` (with `entrypoint` and `description` fields) and a starter `package.sema` file.
- **`sema pkg add` auto-creates `sema.toml`** — no longer requires `sema pkg init` first; the manifest is created on-demand when adding the first dependency.

### Performance

- **13 new VM intrinsic opcodes** — `car`/`first`, `cdr`/`rest`, `cons`, `null?`, `pair?`, `list?`, `number?`, `string?`, `symbol?`, `length`, `append` (2-arg), `get` (2-arg), `contains?` (2-arg) compiled as inline opcodes, eliminating global hash lookup, `Rc` downcast, and argument allocation. Total intrinsified operations: 23. **deriv: 1,123ms → 879ms (1.28×), closure-storm: 1,135ms → 1,029ms (1.10×).**
- **Constant folding optimizer** — new `optimize.rs` pass between lowering and resolution. Folds constant arithmetic, comparisons, boolean ops, `if`/`and`/`or` with constant tests, and dead constants in `begin` blocks.
- **Docker image optimization** — reduced from ~100MB to 11.5MB.

### Changed

- **Replaced hand-rolled CRC32 with `crc32fast`** — switched archive checksum computation to the SIMD-accelerated `crc32fast` crate, removing two duplicate implementations.
- **Package entrypoint renamed** — default package entrypoint changed from `mod.sema` to `package.sema` across resolution logic, CLI discovery, and documentation.
- **Consolidated JSON module** — unified 4 duplicated JSON `Value`↔`serde_json` conversions into canonical `sema-core::json` module (`value_to_json`, `value_to_json_lossy`, `json_to_value`, `key_to_string`). All crates now use the shared implementation.

### Fixed

- **Colorized error output** — errors now display with ANSI colors: red-bold "Error:" prefix, cyan "hint:", yellow "note:", dim stack traces. Includes TTY detection and `NO_COLOR` environment variable support.
- **Source line snippets in errors** — Reader and Eval errors now show Rust-style source context with `-->` location markers and `^` caret pointers.
- **Type errors show offending values** — type errors now display the actual value, e.g., `expected string, got integer (42)`, with 40-character truncation for large values.
- **Arity errors show call context** — arity mismatch errors now include a `note: in: (expr...)` showing the original call form.
- **Stack overflow hints** — "maximum eval depth exceeded" errors now suggest checking for infinite recursion, using TCO, or the `do` form.
- **Mismatched bracket detection** — the reader now specifically detects and reports mismatched bracket types (e.g., `[1 2 3)`) with helpful hints.
- **VM match guard fallthrough** — fixed bytecode compiler bug where match clauses with guards returned nil instead of falling through to the next clause when the pattern itself failed to match.
- **Constant folding division semantics** — `(/ 3 2)` now correctly folds to `1.5` instead of `1`, matching VM runtime behavior.
- **VM prompt/message parity** — prompt and message special forms now build values directly instead of delegating, matching tree-walker output.
- **`value_to_json_lossy` recursion** — fixed to properly traverse nested structures so NaN values inside maps/lists become `null` locally instead of stringifying the entire structure.
- **VFS package import resolution** — VFS is now checked before filesystem resolution, allowing bundled executables to resolve embedded package imports. Transitive imports within packages resolve correctly via synthetic `__entry__` path tracking. Package VFS keys use portable relative paths instead of absolute filesystem paths.
- **VFS thread safety** — VFS backend switched from `RwLock<Option<...>>` to `OnceLock`, eliminating locking overhead on reads, preventing lock poisoning, and enforcing write-once semantics.
- **Git checkout in package manager** — removed erroneous `--` separator that caused refs to be interpreted as file paths.

### Documentation

- **Formatter docs** — dedicated `formatter.md` page covering usage, configuration, formatting rules, and decorative alignment. CLI reference updated with `sema fmt` section.
- **Package manager guide** — new documentation page covering `sema pkg` commands, `sema.toml` manifest format, and package authoring.
- **TOML stdlib reference** — new documentation page for `toml/decode` and `toml/encode`.
- **Prompt & conversation docs** — restructured `prompts.md` and `conversations.md` with workflow examples and accurate API descriptions.
- **Static file serving docs** — documented `http/file` and `:static` route support in web server reference.
- **KV store reference** — expanded with implementation details and examples.
- **`sema compile` vs `sema build` clarification** — added info callout to CLI docs explaining that `sema compile` does not bundle dependencies. Updated executable format docs with VFS path convention examples for git-style and registry packages.
- **Sema syntax highlighting** — custom Shiki grammar for VitePress and updated TextMate grammar.
- **Performance roadmap** — tiered optimization plan with measured results and status tracking.
- **Feature comparison matrix** — Sema vs SBCL, Racket, Guile, Chez, Clojure, Janet, Fennel.
- **Web server docs** — routing, middleware patterns, SSE/WebSocket examples.
- **Updated getting started** — destructuring, match, and modern syntax examples.

### Internal

- **Dual-eval test infrastructure** — `dual_eval_tests!` and `dual_eval_error_tests!` macros for testing both tree-walker and VM in a single test definition.
- **Playground improvements** — draggable splitters, VFS explorer integration, file upload, example reorganization, CSS tooltips on all controls, VFS backend toggle moved to Files panel.
- **CI improvements** — example and bytecode smoke tests, VM examples smoke test.

## 1.10.0

### Added

- **JavaScript embedding library (`@sema-lang/sema`)** — full-featured npm package for embedding Sema in web applications. Wraps the WASM core (`@sema-lang/sema-wasm`) with a high-level TypeScript API: `SemaInstance.create()`, `eval()`, `evalVM()`, virtual filesystem access, and output capture.
- **Pluggable VFS backends** — the WASM virtual filesystem now supports swappable storage backends: `MemoryBackend` (ephemeral), `LocalStorageBackend`, `SessionStorageBackend`, and `IndexedDBBackend` (production-grade persistence with configurable DB/store names).
- **VFS demo playground** — interactive demo at `playground/vfs-demo/` showcasing all 4 VFS backends with live backend swapping.
- **JS embedding docs** — new "Embedding in JavaScript" page on sema-lang.com with installation, API reference, VFS persistence guide, and framework integration examples.
- **npm publish CI workflow** — GitHub Actions workflow using Trusted Publishing (OIDC provenance) for publishing `@sema-lang/sema-wasm` and `@sema-lang/sema` to npm.

### Performance

- **COW map accessors** — zero-refcount-bump fast paths for `assoc`, `dissoc`, `map-get`, `hashmap/assoc` that mutate in place when the map has a single owner, avoiding clone overhead.
- **Trampoline eval loop** — `call_value` lambda results now run through the trampoline loop, enabling proper TCO for indirect calls.
- **VM stack drain** — replaced `to_vec()` + `truncate()` with direct `drain()` for call argument collection, eliminating an allocation per call.
- **`fold-lines` fast path** — native function calls in `fold-lines` now use the fast-path dispatch, enabling COW optimizations for line-by-line processing.
- **`list/unique` BTreeSet** — switched from O(n²) seen-list to O(n log n) BTreeSet for deduplication.

### Changed

- **WASM stack size** — increased from 5 MB to 16 MB to support deeply recursive programs in the browser.

## 1.9.0

### Performance

- **VM intrinsic recognition** — the bytecode compiler now recognizes calls to common builtins (`+`, `-`, `*`, `/`, `<`, `>`, `<=`, `>=`, `=`, `not`) and emits specialized inline opcodes instead of `CallGlobal`. This eliminates global hash lookup, `Rc` downcast, argument `Vec` allocation, and function pointer dispatch for the most frequent operations. **TAK benchmark: 4,352ms → 1,250ms (−71%). Upvalue-counter: 1,232ms → 450ms (−63%).** The `*Int` opcodes include NaN-boxed small-int fast paths that operate directly on raw `u64` bits without constructing a `Value`.
- **Peephole optimization: `(if (not X) ...)`** — the compiler pattern-matches `(if (not expr) then else)` and emits `JumpIfTrue` instead of `Not` + `JumpIfFalse`, saving one instruction per branch.
- **1BRC benchmark re-run** — all 15 Lisp dialect benchmarks re-run in Docker with Sema VM (`--vm`) included. VM result: 23.1s (11.2x vs SBCL), a 2× speedup over the tree-walker (46.3s). Natively: ~15.9s, competitive with Janet and Guile.

### Fixed

- **VM division semantics** — `vm_div` now returns float results for non-whole integer divisions (e.g., `(/ 7 2)` → `3.5`), matching the stdlib `div` function.
- **VM equality semantics** — `vm_eq` now handles mixed int/float comparison correctly (e.g., `(= 1 1.0)` → `#t`), matching the stdlib `eq` function.
- **OpenAI embedding fallback** — configuring an OpenAI embedding model no longer overwrites the chat provider setting.
- **Path denial error messages** — improved error messages when `--allowed-paths` blocks a file operation.

### Added

- **Span end positions** — `Span` now tracks `end_line` and `end_col` in addition to start positions, enabling precise range highlighting in future tooling (LSP, error reporting). Added `Span::to()` and `Span::with_end()` convenience constructors.
- **Stack traces on unbound variable errors** — the tree-walker now attaches a call stack trace to unbound variable errors for easier debugging.
- **Scoped LLM CLI flags** — CLI flags and environment variables for LLM providers are now scoped to chat vs embeddings, allowing independent configuration of each.

### Internal

- **Named constants** — replaced magic numbers across VM and value system with named constants.
- **Exported `SPECIAL_FORM_NAMES`** — canonical list of special form names exported from `sema-eval` for use by tooling.
- **`make deploy`** — combined website and playground deployment target.

## 1.8.0

### Added

- **Path sandboxing (`--allowed-paths`)** — restrict file operations to specific directories. Paths are canonicalized and lexically normalized to prevent traversal attacks (`../../etc/passwd`). Works with all file functions (`file/read`, `file/write`, `file/list`, `kv/open`, `pdf/*`, etc.) and composes with `--sandbox`. Embedding API: `Interpreter::builder().with_allowed_paths(vec![...])`.
- **WASM VFS quotas** — the browser playground virtual filesystem now enforces limits: 1 MB per file, 16 MB total, 256 files max. Prevents runaway memory usage from scripts.
- **VM compiler depth limit** — the bytecode compiler, resolver, and lowering passes now enforce a recursion depth limit (256), preventing stack overflows from deeply nested expressions. Uses RAII guard for panic-safe cleanup.
- **Benchmarks README** — `benchmarks/README.md` documents how to generate test data and run tree-walker vs VM comparisons.

### Changed

- **Benchmark data files** moved from repo root to `benchmarks/data/` (gitignored). All doc references updated.

### Security

- **Fixed path traversal bypass** — `check_path` fallback for nonexistent paths now lexically normalizes `..` segments before the `starts_with` check, closing a bypass where `allowed/nonexistent/../../escape.txt` could escape the allowed directory.
- **Fixed WASM VFS quota bypass** — `file/write-lines` was missing quota enforcement and `VFS_TOTAL_BYTES` tracking. All VFS mutators now use saturating arithmetic to prevent underflow.

## 1.7.0

### Added

- **Bytecode serialization (`.semac`)** — compile Sema source to a binary bytecode format for faster loading and source-free distribution. The format uses a 24-byte header with magic number `\x00SEM`, a deduplicated string table, function table, and main chunk section. See [Bytecode File Format](https://sema-lang.com/docs/internals/bytecode-format.html) for the full spec.
- **`sema compile` subcommand** — compile `.sema` source files to `.semac` bytecode. Supports `-o` for custom output path and `--check` for validation without execution.
- **`sema disasm` subcommand** — disassemble `.semac` files to human-readable text or structured JSON (`--json`).
- **Auto-detect `.semac` files** — running `sema script.semac` automatically detects the magic number and executes via the VM, no `--vm` flag needed.
- **Embedding API: `load_file` and `preload_module`** — `Interpreter::load_file("prelude.sema")` evaluates a file with definitions persisting in the global environment. `Interpreter::preload_module("name", source)` caches a module so `(import "name")` resolves without disk access.
- **Tree-sitter grammar** — full `tree-sitter-sema` grammar with external scanner for nestable block comments, 46 tests across 5 categories. Published to `helgesverre/tree-sitter-sema` mirror repo.
- **Zed editor extension** — syntax highlighting, Go to Symbol (`define`, `defun`, `defmacro`, `defagent`, `deftool`), auto-indentation, bracket matching, and "Run Sema File" task.
- **Homebrew tap** — `brew install helgesverre/tap/sema-lang`.
- **cargo-dist** — automated multi-platform binary releases for Linux (x86_64/aarch64), macOS (Intel/Silicon), and Windows.
- **`llms.txt` and `llms-full.txt`** — LLM-friendly documentation index and full concatenated docs for context ingestion.
- **Smoke test suite** — `make smoke-bytecode` runs all 66 examples through compile → disasm → run (65/66 pass).
- **Link checker CI** — lychee-based link checking workflow and `make lint-links` target.

### Performance

- **VM dispatch loop restructuring** — two-level loop with cached frame locals, raw pointer bytecode reads, direct u8 matching, and deferred PC writeback.
- **Lazy upvalue allocation** — `open_upvalues` deferred until `MakeClosure` actually captures a local, eliminating heap allocation for non-capturing recursive functions.
- **Rc avoidance in call dispatch** — NaN-boxed tag peek (`raw_tag()`, `as_native_fn_ref()`) identifies callables without Rc refcount bumps. Eliminates 60M+ refcount operations on `tak` benchmark (31.8M calls).
- **Specialized opcodes `LoadLocal0..3`, `StoreLocal0..3`** — single-byte zero-operand instructions for the first four local variable load/store slots, eliminating 2-byte operand decode.
- **Fused `CallGlobal` opcode** — combines `LOAD_GLOBAL` + `CALL` into a single instruction for non-tail calls to global functions. Avoids pushing/popping the function value on the stack and uses a direct call path (`call_vm_closure_direct`) that skips the function-slot convention entirely.
- **Global lookup cache** — 16-entry direct-mapped cache with versioned `Env`, avoiding `RefCell` borrow and hashmap lookup on hot global reads.
- **Raw stack operations for integer arithmetic** — `AddInt`, `SubInt`, `MulInt`, `LtInt`, `EqInt` operate directly on raw u64 bits, bypassing Clone/Drop.
- **Benchmark results**: `tak` 9% faster (4.77s → 4.35s), `deriv` 6% faster (1.53s → 1.45s), `upvalue-counter` 15% faster (1.44s → 1.23s). VM remains 2–4× faster than tree-walker across all benchmarks.

### Security

- **Bytecode hardening** — safe Spur conversion (no unsafe transmute), section boundary enforcement, recursion depth limits (128), DoS allocation limits, operand bounds validation, reserved header field checks, string table index 0 validation, section payload consumption verification.

### Documentation

- **CLI reference** — documented `compile`, `disasm`, `--vm`, `--check`, `--json` flags.
- **Bytecode format spec** — updated status from "Design Phase" to "Implemented (Alpha)".
- **OG social preview images** — branded images for website, playground, and GitHub.
- **Editor support** — updated Helix config for native tree-sitter-sema grammar; documented Zed extension.

### Internal

- **crates.io publish workflow** — automated publishing of all 7 workspace crates in dependency order on version tag push.
- **Subtree split CI** — auto-syncs `editors/tree-sitter-sema/` to mirror repo on push to main.
- **Test coverage** — 36+ new unit/integration tests for error types, LLM types, JSON, KV, bytevectors, and bytecode serialization (37 serialization tests).

## 1.6.0

### Added

- **Pretty-printing** — `pprint` builtin and `pretty_print(value, max_width)` in `sema-core`. Smart line-breaking for nested maps/lists with 2-character indentation. Used by REPL and playground for result display.
- **Context module** (15 functions) — ambient key-value context that flows through execution: `context/set`, `context/get`, `context/has?`, `context/remove`, `context/pull`, `context/all`, `context/merge`, `context/clear`, `context/with` (scoped overrides), `context/push`, `context/stack`, `context/pop` (named stacks), `context/set-hidden`, `context/get-hidden`, `context/has-hidden?`. Context auto-appends to `log/info`, `log/warn`, `log/error` output.
- **PDF processing module** (4 functions) — `pdf/extract-text`, `pdf/extract-text-pages`, `pdf/page-count`, `pdf/metadata`. Pure-Rust via `pdf-extract` and `lopdf` crates, sandboxed under `FS_READ`.
- **21 new string functions** — `string/after`, `string/after-last`, `string/before`, `string/before-last`, `string/between`, `string/chop-start`, `string/chop-end`, `string/ensure-start`, `string/ensure-end`, `string/replace-first`, `string/replace-last`, `string/remove`, `string/take`, `string/snake-case`, `string/kebab-case`, `string/camel-case`, `string/pascal-case`, `string/headline`, `string/words`, `string/wrap`, `string/unwrap`.
- **Text utilities** — `text/excerpt` (snippet extraction around a search term with omission markers), `text/normalize-newlines` (convert `\r\n`/`\r` to `\n`).
- **Async HTTP in WASM playground** — `http/get`, `http/post`, `http/put`, `http/delete`, `http/request` now work in the browser playground via a replay-with-cache strategy (uses browser `fetch()` API).
- **`check_arity!` macro** — reduces boilerplate in stdlib function implementations.

### Changed

- **WASM stack tuning** — `MAX_EVAL_DEPTH` lowered to 256 for `wasm32` targets; `.cargo/config.toml` sets 64MB WASM linear memory stack.
- **Stdlib function count** — increased from ~370 to ~460+ registered functions.

### Internal

- **Website docs** — added documentation pages for Context, PDF processing, Playground & WASM HTTP support, and 21 new string functions. Updated stdlib index and quick reference tables.

## 1.5.0

### Breaking

- **`llm/with-budget` demoted from special form to function** — now takes a thunk like the other `llm/with-*` functions: `(llm/with-budget {:max-cost-usd 0.50} (lambda () ...))` instead of `(with-budget {:max-cost-usd 0.50} body...)`.

### Added

- **LLM pipeline & resilience primitives** — `llm/with-cache`, `llm/with-budget`, `llm/with-rate-limit`, `llm/with-fallback`, `llm/cache-stats`, `llm/cache-clear`, `llm/cache-key` for response caching, cost limits, rate limiting, and provider fallback chains.
- **LLM utility functions** — `llm/compare`, `llm/summarize`, `llm/token-count`, `llm/token-estimate`, `llm/default-provider`, `llm/providers`.
- **Vision support** — `llm/extract-from-image` for vision-based structured data extraction.
- **Vector store** — in-memory vector store with `vector-store/create`, `vector-store/add`, `vector-store/search`, `vector-store/delete`, `vector-store/count`, `vector-store/save`, `vector-store/open`, and vector math functions (`vector/cosine-similarity`, `vector/dot-product`, `vector/distance`, `vector/normalize`, `embedding/list->embedding`).
- **Text processing module** (15 functions) — `text/chunk`, `text/chunk-by-separator`, `text/split-sentences`, `text/clean-whitespace`, `text/strip-html`, `text/truncate`, `text/word-count`, `text/trim-indent`, `prompt/template`, `prompt/render`, `document/create`, `document/text`, `document/metadata`, `document/chunk`.
- **Key-value store** (6 functions) — persistent JSON-backed store with `kv/open`, `kv/set`, `kv/get`, `kv/delete`, `kv/keys`, `kv/close`. Sandboxed under `FS_WRITE`.
- **`retry` function** — exponential backoff retry with configurable `:max-attempts`, `:base-delay-ms`, and `:backoff` multiplier.
- **New examples** — `examples/llm/test-pipeline.sema`, `test-text-tools.sema`, `test-vector-store.sema`, `test-kv-store.sema` showcasing the new primitives.

### Changed

- **Playground build system** — examples are now separate `.sema` files in `playground/examples/` (7 categories, 46 files), injected at build time via an esbuild-based build script (`playground/build.mjs`). Replaced 2,351 lines of inline JS with a clean file-per-example structure.
- **Special forms reorganized** — sorted into logical groups (Core language, Modules, LLM primitives) with alphabetical ordering within each group.

### Internal

- **Website docs** — added documentation pages for caching, resilience, vector stores, KV stores, text processing, embeddings, and cost management.
- **Makefile** — `playground-build` now runs `node build.mjs` after wasm-pack.

## 1.4.0

### Changed

- **NaN-boxed Value type** — `Value` is now an 8-byte NaN-boxed `struct Value(u64)` instead of a 24-byte enum. All values are encoded in IEEE 754 quiet NaN payload space:
  - **Immediates** (zero heap allocation): Nil, Bool, Char, Symbol(`Spur`), Keyword(`Spur`), small integers (±17.5 trillion range)
  - **Heap types**: `Rc<T>` pointer stored in 45-bit payload (pointer >> 3, using 8-byte alignment)
  - **Floats**: stored as raw `f64` bits with canonical quiet NaN for NaN values
  - Pattern matching uses `val.view()` → `ValueView` enum; direct accessors (`as_int()`, `as_str()`, etc.) still work
  - VM mode sees **8-12% speedup** from better cache locality; tree-walker sees 9-16% regression from `view()` overhead (acceptable — VM is the future execution path)
  - Memory (RSS) reduced ~5-10% across all benchmarks

### Fixed

- **Dangling pointer UB in `as_bytevector()`/`as_record()`** — `borrow_rc()` created a stack-local `ManuallyDrop<Rc<T>>` and returned a reference into it. Fixed to use `borrow_ref()` directly.
- **Clippy lints for Rust 1.93** — fixed `manual_div_ceil` and `doc_overindented_list_items` warnings in `sema-wasm`.

### Internal

- **VM dispatch loop optimization** — tightened the main `run()` loop and fixed a bug in `call_vm_closure` argument copying.
- **Cross-language benchmark programs** — added Janet and Steel equivalents of `tak` and `nqueens` benchmarks for comparing against other Lisp implementations.
- **VM performance roadmap** — `docs/plans/2026-02-17-vm-performance-roadmap.md` analyzing the 7.8x gap vs Janet with 6-phase optimization plan.

## 1.3.0

### Added

- **Bytecode VM (preview)** — full bytecode compiler and virtual machine, opt-in via `--vm` CLI flag. The VM compiles Sema source through macro expansion → CoreExpr lowering → slot resolution → bytecode compilation → VM execution. Passes 173 unit tests, 130 integration tests, and all 44 examples. Key features:
  - **Same-VM closure execution** — VM closures carry an opaque payload on `NativeFn`; calling a closure pushes a `CallFrame` on the same VM instead of creating a fresh `VM::new()`, eliminating native stack growth.
  - **True tail-call optimization** — `tail_call_vm_closure` reuses the current frame, enabling 100K+ depth tail recursion.
  - **Named-let desugaring** — named `let` is desugared to `letrec` + `lambda` in the lowering pass, simplifying the compiler and fixing self-reference injection bugs.
  - **`delay` lowered to thunk** — `delay` compiles to a zero-arg lambda that captures the lexical environment, ensuring delayed expressions see VM locals.
  - **NativeFn fallback interop** — closures passed to stdlib higher-order functions (map, filter, etc.) go through a NativeFn wrapper, maintaining compatibility with `sema-stdlib` which depends on `sema-core`, not `sema-vm`.
- **New crate: `sema-vm`** — bytecode compiler, resolver, and stack-based virtual machine. Dependency flow: `sema-core ← sema-reader ← sema-vm ← sema-eval`.

### Fixed (VM)

- **Self-ref injection corrupting locals** — `make_closure` no longer writes NativeFn self-references into local slots for all named functions; named-let desugaring eliminates the issue entirely.
- **Missing arity checking** — NativeFn wrapper now performs strict arity validation instead of silently filling missing args with Nil.
- **Recursive inner define** — resolver allocates local slots before resolving RHS, fixing `(define (f) (define (g) (g)) (g))`.
- **`delay`/`force` not capturing lexical vars** — `delay` now lowers to a zero-arg lambda thunk that captures the lexical environment.
- **`__vm-import` selective import** — selective names list symbols are now spread individually in the reconstructed import form.

## 1.2.2

### Internal

- **Lambda/Macro params use interned `Spur` handles** — `Lambda.params`, `Lambda.rest_param`, `Lambda.name`, and the corresponding `Macro` fields changed from `String` to `Spur` (interned u32 handles). Parameter names are now interned once at lambda creation time instead of on every function call. This is a structural change in preparation for the bytecode VM, where the compiler needs `Spur` param names for local slot metadata.

## 1.2.1

### Internal

- **Eliminated mini-eval** — deleted the 620-line duplicated evaluator from `sema-stdlib/src/list.rs`. All stdlib higher-order functions (`map`, `filter`, `foldl`, `sort-by`, etc.) and file streaming functions (`file/fold-lines`, `file/for-each-line`) now call through the real evaluator via a callback architecture in `sema-core`. Net change: **-751 lines**.
- **Callback architecture** — `sema-core` provides thread-local `eval_callback` and `call_callback` functions, registered by `sema-eval` during interpreter initialization. This replaces the mini-eval while preserving the dependency constraint (`sema-stdlib` cannot depend on `sema-eval`).
- **Evaluator fast-path optimizations** — self-evaluating forms (Int, Float, String, Symbol, etc.) now skip depth tracking, step counting, and trampoline setup entirely. Deferred cloning in the trampoline avoids unnecessary `Value::clone()` and `Env::clone()` on non-TCO calls. Thread-local shared `EvalContext` eliminates per-call allocations in stdlib callbacks.
- **Public `call_value` API** — new function in `sema-eval` for calling any callable `Value` (Lambda, NativeFn, Keyword) with evaluated arguments, used by stdlib and LLM builtins.

## 1.2.0

### Added

- **`--sandbox` CLI flag** — restrict dangerous operations at runtime. Supports capability groups (`shell`, `fs-read`, `fs-write`, `network`, `env-read`, `env-write`, `process`, `llm`), presets (`--sandbox=strict`, `--sandbox=all`), and comma-separated denylists (`--sandbox=no-shell,no-network`). Sandboxed functions remain registered but return `PermissionDenied` errors when invoked.
- **`Sandbox` / `Caps` embedding API** — `InterpreterBuilder::with_sandbox(Sandbox::deny(Caps::SHELL.union(Caps::NETWORK)))` for fine-grained control when embedding Sema in Rust applications.
- **`PermissionDenied` error variant** — new structured error type for sandbox violations, catchable with `try`/`catch`.
- **REPL tab completion** — tab-complete built-in function names, special forms, user-defined bindings, and REPL commands. Powered by rustyline's `Completer` trait.
- **`,builtins` REPL command** — list all built-in function names, sorted alphabetically.
- **`llm/extract` schema validation** — new `:validate true` option checks that extracted data matches the schema (key presence and type matching).
- **`llm/extract` retry on mismatch** — new `:retries N` option re-sends the request when validation fails, feeding errors back to the LLM.

### Editor Support

- **VS Code** — TextMate grammar extension with syntax highlighting, bracket matching, comment toggling, and indentation support.
- **Vim / Neovim** — Vimscript plugin with syntax highlighting, filetype detection, and Lisp-aware indentation (`lispwords`).
- **Emacs** — `sema-mode` major mode with syntax highlighting, buffer-local indentation, REPL integration, imenu, and electric pairs.
- **Helix** — tree-sitter highlight queries (on Scheme grammar), text objects, and indentation support.
- All four editors highlight the full standard library (350+ functions), special forms, keyword literals, character literals, LLM primitives, and threading macros.

### Documentation

- **Sandbox docs** — new [CLI sandbox reference](https://sema-lang.com/docs/cli.html#sandbox) and updated [embedding guide](https://sema-lang.com/docs/embedding.html) with sandbox examples.
- **Editor support page** — new [sema-lang.com/docs/editors](https://sema-lang.com/docs/editors.html) with installation instructions for all four editors.
- **Shell completions page** — new [sema-lang.com/docs/shell-completions](https://sema-lang.com/docs/shell-completions.html) with setup instructions for bash, zsh, fish, elvish, and PowerShell.
- **Architecture decisions** — new `docs/decisions.md` documenting naming conventions, Rc cycle behavior, sandbox system, evaluator callback architecture, package system plans, and LSP roadmap.
- **LSP server design** — new `docs/plans/2026-02-16-lsp-server.md` with 4-phase implementation plan using `tower-lsp`.
- **String docs reorganized** — `string/` namespaced functions now lead; legacy Scheme names grouped under "Scheme Compatibility Aliases".

## 1.1.0

### Added

- **`llm/define-provider`** — define LLM providers entirely in Sema code. The `:complete` function receives a request map (`:model`, `:messages`, `:max-tokens`, `:temperature`, `:system`, `:tools`, `:stop-sequences`) and returns a string or a response map with `:content`, `:usage`, `:tool-calls`, and `:stop-reason`. Supports closures, error propagation via `try`/`catch`, and tool-calling agents.
- **OpenAI-compatible provider fallback** — `llm/configure` with any unknown provider name plus `:api-key` and `:base-url` now registers it as an OpenAI-compatible endpoint. Works with Together AI, Azure OpenAI, Fireworks, vLLM, LiteLLM, and any other OpenAI-compatible service.
- **Tool-call responses from Lisp providers** — Lisp-defined providers can return `:tool-calls` in their response maps, enabling tool-calling agents to work with custom providers.

## 1.0.1

### Improved

- **Structured error messages** — errors now support `.with_hint()` and `.with_note()` for actionable suggestions and context. Reader errors show human-readable token names instead of Rust debug format (e.g. `expected \`)\`, got \`]\``instead of`RParen`/`RBracket`).
- **Better error spans** — unterminated lists/vectors/maps now point to the opening delimiter instead of `0:0`.
- **Contextual hints on common errors** — unmatched delimiters, prefix operators without expressions (`'`, `` ` ``, `,`, `,@`), "not callable" errors, and bare `#` all include actionable hints.

### Changed

- **README rewritten** — leads with LLM features (coding agent example) instead of generic Lisp reference. Slimmed from ~1000 lines to ~220, deferring full reference to [sema-lang.com/docs](https://sema-lang.com/docs/).

### Added

- **Favicon** — SVG favicon for both the website and playground.
- **SEO meta tags** — Open Graph and Twitter card tags on both the website and playground.
- **Playground link** — added to website navbar.
- **Playground syntax highlighting** — state-machine tokenizer highlighting keywords, `:keyword` literals, strings, comments, numbers, booleans, and parentheses. Covers all 36 special forms, threading macros, and LLM primitives.

### Fixed

- Suppressed unused `ctx` warning in WASM `with-budget` path.

## 1.0.0

### Changed

- **Explicit `EvalContext`** — all thread-local eval state replaced with an `EvalContext` struct threaded through the evaluator. Multiple independent `Interpreter` instances per thread are now possible.
- **`NativeFn` signature** — now takes `(&EvalContext, &[Value])` with `simple`/`with_ctx` constructors.
- **`EvalCallback` in sema-llm** — updated to accept `&EvalContext`.

### Added

- `EvalContext` defined in `sema-core/src/context.rs`, owned by `Interpreter`.
- `InterpreterBuilder` defaults: `stdlib=true`, `llm=true`.

### Fixed

- Stale counts in docs: 39 special forms (was 33), 19 modules (was 17).

## 0.9.1

### Added

- **`string->float`** — new builtin for direct string-to-float conversion, avoiding the `(float (string->number ...))` roundtrip.

### Performance

- **`vector` in mini-eval** — `(vector ...)` calls now bypass the full trampoline evaluator in hot paths.
- **`string->float` in mini-eval** — fast-path evaluation for `string->float` in the mini-evaluator.
- **`let*` flattening** — using `let*` instead of nested `let` reduces environment allocations (3 per iteration → 1 in the 1BRC benchmark).
- **1BRC benchmark: 12.6s → 9.6s native** (24% faster), 17.9s → 15.4s under Docker emulation (14% faster).

## 0.9.0

### Added

- **Dynamic LLM pricing** — pricing data is now fetched from [llm-prices.com](https://www.llm-prices.com) during `(llm/auto-configure)` and cached at `~/.sema/pricing-cache.json`. Falls back to built-in estimates when offline. Custom pricing via `(llm/set-pricing)` always takes priority.
- **`llm/pricing-status`** — new builtin to inspect which pricing source is active and when it was last updated.
- **WASM playground** — browser-based Sema interpreter with categorized examples and file tree sidebar.

### Performance

- **Env bindings switched from `BTreeMap` to `hashbrown::HashMap`** — variable lookups are now O(1) amortized instead of O(log n), significantly improving performance on compute-heavy code.
- **Pre-interned special form symbols** — `else`, `catch`, and `export` symbols in `cond`/`try`/`case`/`module` are now compared as integer Spurs instead of allocating strings via `resolve()`.
- **Deferred `CallFrame` file allocation** — `CallFrame.file` now stores `PathBuf` directly instead of eagerly converting to `String` on every function call; string conversion only happens when formatting stack traces (on errors).
- **Lambda self-reference via `Rc::clone`** — recursive named lambdas no longer reconstruct the entire `Lambda` struct (params, body, env) on every call; they reuse the existing `Rc<Lambda>`.
- **Allocation-free `Display` for `Value`** — `Symbol`, `Keyword`, and `Record` display now use `with_resolved()` (borrows `&str`) instead of `resolve()` (allocates `String`).
- **Step-limit check hoisted out of trampoline loop** — `EVAL_STEP_LIMIT` TLS read moved before the loop so it's read once per eval instead of every iteration.
- **Optimized release profile** — added `lto = "thin"`, `codegen-units = 1`, `panic = "abort"` for faster release binaries; separate `release-with-debug` profile for profiling.

### Fixed

- **Unicode-safe string operations** — `string-length`, `substring`, `length`, and `count` now count characters (Unicode scalar values) instead of bytes. `string/pad-left` and `string/pad-right` use character width for padding. Previously, `(string-length "héllo")` returned 6 (bytes); now it correctly returns 5 (characters). `substring` no longer panics on multi-byte character boundaries.
- **Display panic on multi-byte strings** — Fixed `truncate` in `Value::Message` display to use character-based truncation instead of byte slicing, which could panic on messages containing emoji or non-ASCII text.
- **HashMap support in map operations** — `dissoc`, `merge`, `map/entries`, `map/map-vals`, `map/filter`, `map/select-keys`, `map/map-keys`, and `map/update` now accept both sorted maps and hashmaps, preserving the input type. Previously these functions only worked on sorted maps.
- **Stale Groq pricing** — Groq models are no longer hardcoded as free ($0.00); updated to current estimates.
- **Budget enforcement with unknown pricing** — now warns once instead of silently skipping cost tracking when pricing is unavailable for a model.

## 0.8.0

### Added

- **Embedding API** — `sema` crate now exposes a library with `Interpreter`, `InterpreterBuilder`, and `register_fn()` for embedding Sema as a scripting engine in Rust applications. Builder toggles for stdlib (`with_stdlib`) and LLM (`with_llm`) with sensible defaults.
- **Persistent defines** — `eval_str_in_global` / `eval_in_global` methods on the evaluator so that `define` persists across multiple eval calls (used by the embedding API).
- **Embedding documentation** — new docs page with quick start, builder config, native function registration, a data pipeline example, and threading model notes.

### Fixed

- **Integer overflow panics** — stdlib arithmetic (`+`, `-`, `*`), `abs`, `pow`, `math/quotient`, `math/gcd`, `math/lcm` now use wrapping operations instead of panicking on overflow.
- **"No global state" claim** — removed misleading claim from README and docs; Sema uses a thread-local string interner.

## 0.7.0

### Added

- **String escape sequences** — R7RS-style `\x<hex>;` hex escapes, `\uNNNN` (4-digit), `\UNNNNNNNN` (8-digit) Unicode escapes, and `\0` null escape in string literals. Enables producing any Unicode character including ESC (`\x1B;`) for ANSI terminal codes.
- **Agent message history** — `(agent/run agent msg {:messages history})` returns `{:response "..." :messages [...]}`, enabling multi-turn agent conversations with persistent message history.
- **VitePress documentation site** — Full documentation website with 30+ pages covering stdlib, LLM primitives, language reference, and CLI. All code examples verified against the interpreter.

### Fixed

- **Shell single-string commands** — `(shell "ls -la")` now correctly invokes the system shell for command parsing instead of treating the entire string as an executable name.
- **Tool argument ordering** — `deftool` handlers now receive arguments in lambda declaration order instead of alphabetical BTreeMap key order, fixing mismatches when parameter names aren't alphabetically sorted.

## 0.6.1

### Added

- **System introspection** — `sys/tty` (TTY device name), `sys/pid` (process ID), `sys/arch` (CPU architecture), `sys/os` (OS name), `sys/which` (find executable in PATH), `sys/elapsed` (monotonic nanosecond timer)

### Fixed

- `test_sys_interactive` no longer flaky in environments with a TTY attached

## 0.6.0

### Added

- **List operations** — `list/shuffle` (random reorder), `list/split-at` (split at index), `list/take-while` / `list/drop-while` (predicate-based prefix ops), `list/sum` (numeric sum), `list/min` / `list/max` (extrema), `list/pick` (random element), `list/repeat` / `make-list` (create n copies), `iota` (SRFI-1 integer sequence generator)
- **String operations** — `string/map` (map function over characters), `string/capitalize` (capitalize first letter), `string/reverse`, `string/title-case`
- **Math aliases** — `modulo` (alias for mod), `expt` (alias for pow), `ceiling` (alias for ceil), `truncate`
- **Type conversion** — `number->string`
- **11 new example programs** — Gabriel benchmarks, ASCII art, Perlin noise, Game of Life, lorem ipsum, maze generator, Mandelbrot set, and more
- **System introspection** — `sys/tty` (TTY device name), `sys/pid` (process ID), `sys/arch` (CPU architecture), `sys/os` (OS name), `sys/which` (find executable in PATH), `sys/elapsed` (monotonic nanosecond timer)

### Changed

- Stdlib builtin count increased from ~280 to ~350+ registered functions

## 0.5.0

### Added

- **Character comparison predicates** — R7RS `char=?`, `char<?`, `char>?`, `char<=?`, `char>=?` and case-insensitive `char-ci=?`, `char-ci<?`, `char-ci>?`, `char-ci<=?`, `char-ci>=?`.
- **`define-record-type`** — R7RS record types with constructors, type predicates, and field accessors. `record?` predicate. `type` returns record type name as keyword for records.
- **Bytevectors** — `Value::Bytevector` with `#u8(1 2 3)` reader syntax. `make-bytevector`, `bytevector`, `bytevector-length`, `bytevector-u8-ref`, `bytevector-u8-set!` (COW), `bytevector-copy`, `bytevector-append`, `bytevector->list`, `list->bytevector`, `utf8->string`, `string->utf8`, `bytevector?`.

## 0.4.0

### Added

- **Character type** — First-class `#\a` syntax with named characters (`#\space`, `#\newline`, `#\tab`, `#\return`, `#\nul`). `char?`, `char-alphabetic?`, `char-numeric?`, `char-whitespace?`, `char-upper-case?`, `char-lower-case?` predicates. `char-upcase`, `char-downcase` case conversion. `char->integer`, `integer->char`, `char->string`, `string->char`, `string->list`, `list->string` conversions.
- **Lazy evaluation** — `delay`/`force` with memoized promises. `promise?` and `promise-forced?` predicates. `force` on non-promise passes through (R7RS compatible).
- **Proper `do` loop** — R7RS `(do ((var init step) ...) (test result ...) body ...)` with parallel variable assignment. Replaces previous `do` alias for `begin`.
- **Car/cdr compositions** — 12 shortcut functions: `caar`, `cadr`, `cdar`, `cddr`, `caaar`, `caadr`, `cadar`, `caddr`, `cdaar`, `cdadr`, `cddar`, `cdddr`.
- **Association lists** — `assoc` now dual-purpose: `(assoc key alist)` for alist lookup, `(assoc map key val ...)` for map assoc. New `assq` and `assv` functions.

### Changed

- `string-ref` now returns `Value::Char` instead of a single-character string
- `string/chars` now returns a list of `Char` values instead of single-character strings
- `do` is no longer an alias for `begin` — it is now a proper Scheme iteration form

## 0.3.0

### Performance

- **String interning with `lasso`** — `Value::Symbol` and `Value::Keyword` now store `Spur` (u32 interned key) instead of `Rc<String>`. Symbol/keyword equality is O(1) integer comparison. `Env` bindings keyed by `Spur` for direct lookup without string allocation. Mini-eval special form dispatch uses pre-interned Spur constants — no string matching in hot path.
- **`hashbrown` HashMap variant** — New `Value::HashMap` type backed by `hashbrown::HashMap` with O(1) amortized lookups. `hashmap/new`, `hashmap/get`, `hashmap/assoc`, `hashmap/to-map`, `hashmap/keys`, `hashmap/contains?` builtins. Existing `get`, `assoc`, `keys`, `vals`, `contains?`, `count`, `empty?` also work on HashMaps. COW optimization (Rc::make_mut) applies to HashMap assoc.
- **SIMD byte search with `memchr`** — `string/split` uses SIMD-accelerated `memchr` for single-byte delimiter search.
- **1BRC benchmark: 1580ms → 1340ms** (15% faster for 1M rows)

### Added

- `Value::HashMap` data type — opt-in unordered hash map for performance-critical accumulation
- `hashmap/new`, `hashmap/get`, `hashmap/assoc`, `hashmap/to-map`, `hashmap/keys`, `hashmap/contains?` builtins
- `Hash` implementation for `Value` — enables use as `HashMap` keys
- `intern()`, `resolve()`, `with_resolved()` — string interner API in `sema-core`

### Changed

- `Value::Symbol` stores `Spur` (u32) instead of `Rc<String>` — **breaking if matching on inner type directly**
- `Value::Keyword` stores `Spur` (u32) instead of `Rc<String>` — **breaking if matching on inner type directly**
- `Env::bindings` uses `BTreeMap<Spur, Value>` instead of `BTreeMap<String, Value>`
- `as_symbol()` and `as_keyword()` now return `Option<String>` instead of `Option<&str>`

### Dependencies

- Added `lasso` 0.7 (string interning) to `sema-core`
- Added `hashbrown` 0.15 (fast HashMap) to `sema-core` and `sema-stdlib`
- Added `memchr` 2 (SIMD byte search) to `sema-stdlib`

## 0.2.1

### Performance

- **Optimized `file/fold-lines`** — reuses lambda env and moves accumulator (no Rc clone per line)
- **Optimized `file/for-each-line`** — reuses lambda env instead of creating a new one per line
- **Inlined hot-path builtins in mini-eval** — `assoc`, `get`, `nil?`, `+`, `=`, `min`, `max`, `first`, `nth`, `float`, `string/split`, `string->number` bypass Env lookup and NativeFn dispatch
- **Zero-clone `assoc`** — uses `Env::take` + `Rc::make_mut` to mutate maps in-place when refcount is 1
- **Added `Env::take()`** — removes and returns a binding from the current scope, enabling move semantics

### Internal

- Made `sema_eval_value` public in sema-stdlib for reuse by `file/fold-lines` and `file/for-each-line`

## 0.2.0

### Added

- **`defun` alias** — Common Lisp-style `(defun name (params) body)` as alias for `define`
- **`sema ast` subcommand** — Parse source and display AST as tree or JSON (`--json`)
- **Slash-namespaced LLM accessors** — `tool/name`, `agent/system`, `prompt/messages`, `message/role`, etc. (legacy names still work)
- **Provider introspection** — `llm/set-default`, `llm/list-providers`, `llm/current-provider`
- **Budget control** — `llm/set-budget`, `llm/clear-budget`, `llm/budget-remaining`
- **Gemini and Ollama tool-call support**
- **Auto rate-limit retry** — 3 attempts with exponential backoff
- **HTTP timeouts** — 120s on all provider requests
- **`conversation/say` options** — accepts optional `{:temperature :max-tokens :system}` map

### Fixed

- Website code examples now use valid, copy-pasteable Sema syntax

## 0.1.0

Initial release — Phases 1-8 complete.

- Scheme-like core with Clojure-style keywords, maps, vectors
- Trampoline-based tail-call optimization
- 226 stdlib builtins across 17 modules
- 29 LLM builtins: completion, chat, streaming, extraction, classification, tool use, agents
- 11 LLM providers: Anthropic, OpenAI, Gemini, Ollama, Groq, xAI, Mistral, Moonshot, Jina, Voyage, Cohere
- Module system with `import`/`export`
- Macros with quasiquote/unquote/splicing
- Error handling with `try`/`catch`/`throw` and stack traces
- REPL with readline, file runner, `-e`/`-p` eval modes
