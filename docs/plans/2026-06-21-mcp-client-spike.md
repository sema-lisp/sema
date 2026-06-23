# MCP Client — spike & scoping (Sema as an MCP *client*)

**Status:** Scoping / design sketch (2026-06-21; auth + HTTP transport added
2026-06-23). Not started. Answers the two questions raised: **how** would it work
and **where** does it live (agent-only vs whole-language)? — and now also: how do
we reach **authenticated remote servers** (Asana, Linear, hosted GitHub, …) and
what is the login/token journey.

## Context: today Sema is an MCP *server* only

`crates/sema-mcp/` exposes Sema **to** external agents (protocol.rs, server.rs,
tools.rs). It already contains the Sema-params ↔ JSON-Schema conversion
(`tools.rs`) that an MCP *client* needs in reverse. There is **no MCP client** —
Sema cannot currently consume external MCP servers' tools.

## Why a client is worth it

MCP is becoming the universal tool protocol. An MCP *client* lets a Sema agent
use the entire external MCP ecosystem (filesystem, GitHub, Slack, browsers,
databases, …) without Sema writing a `deftool` for each. That is a large
strategic multiplier for the agentic story — and it composes with the existing
`deftool`/`defagent`/agent-loop machinery instead of replacing it.

## The "where" decision — layered, with a clear boundary

The instinct to ask "is this just for `defagent` or for the whole language?" is
the right one. Recommendation: **build it as two layers with a hard boundary.**

```
┌─────────────────────────────────────────────────────────────┐
│ Layer 2 — Agent adapter (thin)                               │
│   mcp/tools->sema : turn MCP tool descriptors into the SAME  │
│   value shape `deftool` produces, so `defagent` consumes them│
│   exactly like local tools. NO new agent concepts.           │
├─────────────────────────────────────────────────────────────┤
│ Layer 1 — General MCP client primitive (stdlib namespace)    │
│   mcp/connect, mcp/tools, mcp/call, mcp/close — usable by    │
│   ANY Sema code, agent or not. This is a transport + RPC     │
│   client, nothing agent-specific.                            │
└─────────────────────────────────────────────────────────────┘
```

**Boundary rule:** Layer 1 knows nothing about agents — it's a protocol client,
like `http/*`. Layer 2 is a ~20-line adapter that maps an MCP tool's
`{name, description, inputSchema}` onto a Sema tool value (the same triple
`deftool` builds: name, description, params-map, handler) where the handler is a
closure that calls `mcp/call`. This keeps the agent layer thin and means
non-agent scripts get MCP for free.

This mirrors how the codebase already separates the general `http/*` builtins from
higher-level `llm/*` features that use them.

## Layer 1 — surface sketch

```sema
;; Connect to an MCP server. Transport inferred from the spec.
(define fs (mcp/connect {:command "npx" :args ["-y" "@modelcontextprotocol/server-filesystem" "/tmp"]}))
;; or remote (Streamable HTTP). No auth needed if the server is open or you pass a token:
(define gh (mcp/connect {:url "https://mcp.example.com/mcp"
                         :headers {"Authorization" "Bearer …"}}))   ; bring-your-own-token

;; or a remote server that requires OAuth — login is automatic (browser pops on first use):
(define asana (mcp/connect {:url "https://mcp.asana.com/mcp"}))
;; …or pin an explicit pre-registered client + scopes when the server needs them:
(define asana (mcp/connect {:url "https://mcp.asana.com/mcp"
                            :auth {:client-id "…" :scopes ["default"]}}))

(mcp/tools fs)
; => [{:name "read_file" :description "…" :parameters {…json-schema…}} …]

(mcp/call fs "read_file" {:path "/tmp/notes.txt"})
; => {:content "…"}   (MCP result, normalized to a Sema value)

(mcp/close fs)
```

- `mcp/connect` returns an opaque **handle** (an Rc-backed resource value, like a
  file/stream handle). Connection + `initialize` handshake happen eagerly. For an
  authenticated HTTP server, the first `initialize` may trigger the OAuth flow
  (see **Authentication & authorization** below) before the handshake completes —
  cached tokens make subsequent connects silent.
- `mcp/tools` performs `tools/list`; `mcp/call` performs `tools/call`.
- Errors surface as `SemaError` (reuse the JSON-RPC error mapping). Auth failures
  (`401`, refused consent, sandbox-blocked browser) surface as distinct,
  actionable `SemaError`s with `.with_hint()`.

## Layer 2 — agent integration sketch

```sema
(define fs (mcp/connect {:command "npx" :args ["-y" "@modelcontextprotocol/server-filesystem" "/tmp"]}))

(defagent librarian
  {:model "claude-…"
   :system "You manage files."
   ;; MCP tools become first-class agent tools, indistinguishable from deftool ones:
   :tools (mcp/tools->sema fs)})         ; <- the entire adapter surface
```

`mcp/tools->sema` produces values structurally identical to what `deftool` yields
(name, description, JSON-schema params, handler), so the existing agent loop's
tool-dispatch path needs **zero changes**. Optionally support a convenience
`:mcp-servers [fs gh]` key on `defagent` that calls `mcp/tools->sema` internally.

## Implementation placement & dependency graph

- Put the client in `crates/sema-mcp/` as a new `client/` module (transport +
  JSON-RPC + tool conversion), reusing `protocol.rs` types and inverting
  `tools.rs`'s schema conversion (JSON-Schema → Sema params map).
- Register the `mcp/*` builtins where the binary wires stdlib (the binary already
  composes `sema-stdlib` + `sema-llm`; add `sema-mcp` client builtins alongside).
  This keeps `sema-stdlib` free of an MCP dependency (respects the existing
  "stdlib has no heavy deps" rule).
- Layer 2 (`mcp/tools->sema`) lives wherever the agent/tool value shape is
  defined so it can produce the identical structure.

## Transport

MCP defines **two** standard transports, and the wire format for the remote one
changed: the old **HTTP+SSE** transport (two endpoints, a long-lived `/sse`
channel) was **deprecated in the 2025-03-26 spec** and replaced by **Streamable
HTTP** — a *single* endpoint that takes a POST and either returns JSON or upgrades
the response to an SSE stream, with the session carried in an `Mcp-Session-Id`
header. **Build against Streamable HTTP, not the legacy `/sse` shape.** A
transport trait (`stdio` vs `http`) keeps Layer 1 uniform; `mcp/connect` selects
it from the spec map (`:command` ⇒ stdio, `:url` ⇒ http).

- **Phase 1: stdio** — spawn a subprocess, JSON-RPC over stdin/stdout. Most common
  deployment, simplest to test, **no auth** (the server inherits the environment
  Sema gives it — e.g. a token passed via an `:env` map). Mirror
  `crates/sema-mcp/src/server.rs`'s line-delimited JSON-RPC loop, in reverse.
- **Phase 2: Streamable HTTP** — remote servers. Exact wire contract, lifted from
  the official SDK transports (`typescript-sdk` `streamableHttp.ts`,
  `python-sdk` `streamable_http.py`) so we replicate it precisely:
  - **POST** every JSON-RPC message with `Content-Type: application/json` and
    `Accept: application/json, text/event-stream`. The response is *either*
    `application/json` (single message) **or** `text/event-stream` (an SSE stream
    for long calls) — branch on the `Content-Type`; a `202 Accepted` with no body
    is the valid reply for notifications/responses.
  - **Session:** capture the `Mcp-Session-Id` response header on the `initialize`
    reply and resend it on every subsequent request; on `404` for a stale session,
    restart from `initialize`. Send `DELETE` to end a session.
  - **Protocol version:** read `protocolVersion` from `InitializeResult` and send
    it as the `MCP-Protocol-Version` header thereafter.
  - **Server push (optional):** issue a `GET` with `Accept: text/event-stream` to
    open the server→client SSE channel; reconnect with `Last-Event-ID`.
  - Reuse `sema-llm`'s `http.rs` (thread-local tokio runtime + reqwest 0.13,
    `block_on`) and `sse.rs` (`parse_sse_stream`). Legacy 2024-11-05 HTTP+SSE
    back-compat (POST→4xx→GET-for-`endpoint`-event) is optional; defer.

HTTP is **not** an afterthought: authenticated remote servers are HTTP-only, so
the auth work below depends on the HTTP transport landing.

## Build vs reuse (Rust dependencies) — verified 2026-06-23

The single biggest decision. Two viable native paths; **no external proxy tool
(`mcp-remote`) in either** — that's explicitly out (see "Reference
implementations" below for why it's reference-only).

**Option A — adopt `rmcp` (official Rust MCP SDK), feature-gated.** `rmcp` 1.7.0
ships a full *client* (`serve_client`, `Peer<RoleClient>`), both transports we
need (`TokioChildProcess` for stdio, `StreamableHttpClientTransport` over
reqwest), **and a complete OAuth 2.1 stack** behind its `auth` feature: RFC 9728
PRM discovery, RFC 8414 AS discovery, PKCE S256, RFC 7591 DCR + CIMD fallback,
RFC 8707 resource indicators, auto-refresh, and `403 insufficient_scope`
re-scoping (`OAuthState` state machine, `AuthorizationManager`,
`AuthorizedHttpClient`). **The one gap is token *persistence* (in-memory only) —
we'd supply that ourselves.**
  - **Dependency check (done):** the feared duplicate-reqwest conflict does **not**
    exist — `rmcp` pins `reqwest 0.13.2` and the Sema workspace is already on
    `reqwest = "0.13"` (resolves to 0.13.2) and `axum = "0.8"`. `cargo tree -d`
    shows a single `reqwest v0.13.2`. `rmcp` also uses `oauth2 = "5"` internally.
  - **Cost:** `rmcp`'s client is an async, `Send + Sync`, service-oriented model
    (a background task drives `RunningService<RoleClient>`); Sema is single-threaded
    `Rc` + sync-over-async (`block_on`). Bridging is doable (own the rmcp client
    behind the opaque Rc handle, `block_on` each call on the thread-local runtime)
    but it's the real integration cost, and `rmcp` is a large surface for what we
    use. Pin `>= 1.4.0` (DNS-rebinding CVE GHSA-89vp-x53w-74fx fixed there).

**Option B — hand-roll the transport, reuse small crates for the hard parts
(recommended).** The existing `sema-mcp` *server* is already hand-rolled (not
`rmcp`), the JSON-RPC + schema-conversion code is right there to invert, and this
keeps the single-threaded `block_on` model clean. Don't hand-roll crypto/flows —
reuse:
  - **`oauth2 = "5"`** — PKCE S256 (`PkceCodeChallenge::new_random_sha256`),
    auth-code exchange, refresh. RFC 8707 `resource` via `.add_extra_param(...)`.
    Takes our existing `reqwest::Client` (`.request_async(&client)`) — no new HTTP
    stack. (Note: `oauth2` itself targets reqwest `^0.12` in its optional client;
    we drive it with our own reqwest 0.13 client via the `AsyncHttpClient` impl —
    verify this compiles early, it's the one integration risk in Option B.)
  - **RFC 9728/8414 discovery** — hand-rolled, ~30 lines: two `GET`s to the
    `.well-known/*` endpoints + two `serde` structs. No crate exists for this.
  - **`open = "5"`** — launch the browser (graceful `io::Result`, no panic headless).
  - **loopback callback** — minimal `axum` router (already in-workspace) on
    `127.0.0.1:0`, one `/callback` route → `tokio::sync::oneshot`, then shut down.
  - **`keyring-core = "1"`** + per-platform backends for token storage (below).

**Recommendation: Option B**, matching the codebase's hand-rolled MCP server and
single-threaded model, with Option A as a documented fallback (now de-risked since
reqwest aligns) if the hand-rolled transport proves heavier than expected. This is
the top open question — confirm before M3.

## Authentication & authorization (remote servers)

stdio servers need no auth — they inherit the environment Sema gives the child
process (e.g. a `GITHUB_TOKEN` passed in `:env`). **Authenticated remote servers
(Asana, Linear, hosted GitHub, …) are the real work the original sketch omitted.**
MCP standardised this on **OAuth 2.1** (mandatory PKCE), with the MCP server
acting only as an OAuth *Resource Server* and a separate *Authorization Server*
(the vendor's own OAuth, e.g. Asana's) issuing tokens.

### Architecture: a host-implemented auth store + a spec-driven engine

Every reference client (TS SDK `OAuthClientProvider`, Python SDK `TokenStorage` +
`httpx.Auth`, opencode `McpOAuthProvider`, mcp-remote `NodeOAuthClientProvider`)
splits the same two responsibilities — copy this split:

- **Auth engine (spec mechanics, stateless):** discovery → DCR/CIMD → PKCE →
  build auth URL → exchange code → refresh → re-scope on `403`. This is exactly
  what `rmcp`'s `auth` feature or `oauth2` + ~30 lines of discovery gives us; we
  do **not** hand-write crypto.
- **Auth store (host I/O, stateful) — a small Sema-side trait** mirroring
  `OAuthClientProvider`'s callbacks: `tokens()/save_tokens()`,
  `client_info()/save_client_info()`, `code_verifier()/save_code_verifier()`,
  `state()/save_state()`, `redirect_to_authorization(url)`, plus
  `invalidate(scope)` for self-healing. Sema implements this against its
  keychain/file store (below) and its browser-opener + loopback listener.

This is the seam that keeps Layer 1 a clean protocol client: the engine never
touches disk or the browser; the store never knows OAuth mechanics.

### The journey — two-phase connect (the opencode/SDK pattern)

1. **Unauthenticated probe (phase 1 connect).** `mcp/connect` POSTs `initialize`;
   server answers `401` with `WWW-Authenticate: Bearer resource_metadata="…",
   scope="…"`. The transport extracts `resource_metadata` + `scope` and signals
   "auth required", capturing the half-open transport (opencode's
   `pendingOAuthTransports`).
2. **Discover the authorization server.** Fetch Protected Resource Metadata
   (RFC 9728) at the advertised URL (fallbacks: `/.well-known/oauth-protected-resource`
   path- then origin-variants) → `authorization_servers[]` + canonical `resource`.
   Then fetch *that* server's metadata: try `/.well-known/openid-configuration`
   **then** `/.well-known/oauth-authorization-server` (the SDK order) →
   `authorization_endpoint` / `token_endpoint` / `registration_endpoint` /
   `scopes_supported`. Validate the returned `issuer` matches the URL fetched.
   Cache this (SDK `discoveryState`) so reconnects skip the probes.
3. **Obtain a `client_id`** — priority order from the 2025-11-25 spec: (a)
   **pre-registered** id the user configured (`:auth {:client-id …}`); (b) **CIMD**
   if `client_id_metadata_document_supported` (an HTTPS URL *as* the client_id —
   would mean hosting a doc at e.g. `sema-lang.com`; rare on commercial AS today);
   (c) **Dynamic Client Registration** (RFC 7591 — `token_endpoint_auth_method:
   "none"`, `grant_types: ["authorization_code","refresh_token"]`,
   `redirect_uris: [loopback]`). For Asana/Linear today, expect pre-registration
   via their developer portal. Persist DCR results.
4. **Browser authorization-code flow + PKCE (mandatory).** Generate PKCE (128-char
   verifier, S256), save the verifier + a random `state`, **open the browser** to
   `authorization_endpoint?response_type=code&client_id=…&code_challenge=…&
   code_challenge_method=S256&redirect_uri=…&state=…&scope=…&
   resource=<canonical-mcp-server-uri>` (RFC 8707 `resource` is **mandatory**;
   add `prompt=consent` when requesting `offline_access`).
5. **Capture the redirect on a loopback listener.** Per **RFC 8252**, bind
   `127.0.0.1:<port>` *before* opening the browser; AS redirects to
   `http://127.0.0.1:<port>/callback?code=…&state=…&iss=…`. **Validate `state`**
   (CSRF) and `iss` when present (RFC 9207); reply with a small "you can close this
   tab" page (`window.close()`); shut the listener down. Port strategy (mcp-remote):
   a deterministic preferred port derived from a hash of the server URL, falling
   back to an OS-assigned ephemeral port (`TcpListener::bind("127.0.0.1:0")`) if
   busy; reuse the registered port across runs so DCR `redirect_uris` stay stable.
6. **Exchange + store tokens (phase 2 connect).** POST `code` + PKCE
   `code_verifier` + `redirect_uri` + `resource` to `token_endpoint`; persist
   access + refresh tokens; re-run `initialize` with `Authorization: Bearer …` on
   the captured transport (`transport.finishAuth(code)` in the SDK).
7. **Use, refresh, self-heal.** Send `Authorization: Bearer <token>` on every
   request. On `401`: refresh via the refresh token before re-prompting. On `403
   insufficient_scope`: union prior+granted+challenged scopes and re-authorize. On
   `invalid_grant` at the token endpoint: clear tokens and re-auth; on
   `invalid_client`/`unauthorized_client`: clear *all* creds (incl. DCR) and
   re-register. (Request `offline_access` scope when a refresh token is wanted.)

### Does it open a browser? Headless / sandbox caveat

Yes — step 4 needs a real browser. **Sema has no browser-opening code today** (no
`open`/`webbrowser` crate, no shell-out to `open`/`xdg-open`/`start`). Add one
(the `open` crate is the simplest). But the current environment is often
**headless / CI / sandboxed**, where popping a browser is impossible, so the flow
**must degrade gracefully**: print the authorization URL and have the user open it
manually and paste back the resulting code (a `sema mcp login <url>` one-off
command is the natural home for this). A **device-authorization-grant** path can
be added later for fully headless boxes *if* the server supports it.

### Where do tokens live? (Sema has no config dir today)

Sema currently keeps **no user config/data directory** and reads all secrets from
env vars (`crates/sema-llm/src/builtins.rs`); the only on-disk state is opt-in LLM
cassettes via `SEMA_LLM_CASSETTE`. Remote MCP auth forces Sema's **first
persistent credential store**.

**Per-server stored entry** (schema converged across opencode `mcp-auth.json` and
mcp-remote): `{ tokens: {access, refresh?, expires_at?, scope?}, client_info:
{client_id, client_secret?, issued_at?, secret_expires_at?}, code_verifier?
(transient), state? (transient), server_url }`. **Always store `server_url` and
validate it on read** (opencode's `getForUrl`) so stale creds from a changed
endpoint are ignored and re-auth runs.

**Storage backend — keychain first, `0600` file fallback** (what `gh` / VS Code
do, with the real gotchas surfaced in the research):

- **OS keychain via `keyring-core = "1"`** (the v4 `keyring` umbrella is now just a
  CLI; depend on `keyring-core` + per-platform backend crates:
  `apple-native-keyring-store`, `windows-native-keyring-store`, and on Linux
  `zbus-secret-service-keyring-store` with `linux-keyutils-keyring-store` as the
  daemonless fallback). **Footgun:** keyring calls block — wrap them in
  `spawn_blocking`, never call on the tokio runtime thread (deadlock).
- **`0600` JSON file** (config dir via `directories`, honoring `$XDG_CONFIG_HOME`;
  e.g. `~/.config/sema/mcp-auth.json`) when no keychain is available (headless
  Linux/CI returns `NoStorageAccess` — there is **no** automatic fallback, we
  implement it). Gotchas: call `set_permissions(0o600)` **after** create/truncate
  (`OpenOptions.mode` only applies on first create); `mode()` is a **no-op on
  Windows** (use an ACL or accept the user-profile ACL); print a **visible** warning
  when falling back to plaintext.
- **Multi-process safety:** an advisory file lock keyed by the file path (opencode
  uses one) so two Sema processes don't clobber each other's refresh writes.
- **Never log tokens; never write them world-readable.** `sema mcp logout <url>`
  (or clearing the entry) resets auth state.

### Reference implementations & test oracle (not shipping deps)

Native means **no `mcp-remote` proxy in the shipping path.** It remains useful as
(a) a **reference** for the exact native flow — its `NodeOAuthClientProvider`,
deterministic-port logic, and loopback server are the clearest small example — and
(b) a **manual test oracle** to confirm a given server's OAuth works before we
point Sema's native client at it. Other references worth reading while building:
**opencode** `packages/opencode/src/mcp/` (TS, the closest analogue: provider +
callback server + `mcp-auth.json`), the **official TS/Python SDKs**
(`client/auth.*`, `streamable_http.*`), **`rmcp`** `examples/clients/oauth_client.rs`
+ `docs/OAUTH_SUPPORT.md` (Rust, 1:1 port of the SDK flow), and
**`rust-mcp-stack/oauth2-test-server`** as a local IdP to exercise our flow in CI
without a real provider.

### Capability / sandbox implications

The OAuth flow adds new authority surfaces beyond plain network I/O: spawning a
browser, listening on a loopback socket, and writing a token file. Gate them on
the existing bitset (`crates/sema-core/src/sandbox.rs`, ADR #62):

- HTTP transport ⇒ **`NETWORK`**; token store ⇒ **`FS_WRITE`**; the loopback
  callback listener ⇒ **`NETWORK`** (inbound on localhost); opening a browser ⇒ a
  **`PROCESS`**-adjacent action (it spawns/launches the default browser).
- A sandboxed program lacking these must **fail closed** with a clear, hinted
  `SemaError` — never silently skip auth or fall back to an insecure path.
- Open question (below): does "open a browser" deserve its own capability bit, or
  does it fold into `PROCESS`?

## Security & sandbox boundary (non-negotiable)

Connecting to an MCP server = **spawning a process** and/or **network I/O**, and
the called tools can do anything the server allows. This MUST be gated by the
existing capability bitset (ADR #62):

- `mcp/connect` via stdio requires the process-spawn capability; via URL requires
  the network capability.
- A sandboxed Sema program with neither capability cannot open MCP connections —
  same model as `shell`/`http`.
- Document clearly that MCP tools run with the *server's* authority, not Sema's
  sandbox — connecting to an untrusted MCP server is equivalent to running
  untrusted code. This is a docs + capability-gating concern, called out here so
  it isn't an afterthought.

## Determinism / testing tie-in

MCP `tools/call` is I/O, so it has the same nondeterminism problem as LLM calls.
Design the **cassette** tape format (see `2026-06-21-llm-cassettes.md`) with an
open `kind` field so `"mcp-call"` interactions can be recorded/replayed too — then
agent tests that use MCP tools stay deterministic and offline in CI.

## Milestones

- **M0 — build-vs-reuse decision (gate before M3).** Resolve Option A (`rmcp`) vs
  Option B (hand-roll + `oauth2`). Cheap spikes: (1) confirm `oauth2 = "5"` drives
  our reqwest-0.13 client via `AsyncHttpClient` (the one Option-B risk); (2) sketch
  bridging `rmcp`'s async `RunningService` behind a `block_on` Rc handle (the one
  Option-A risk). Pick, then proceed. *Acceptance:* a one-page decision recorded
  here with the spike result.
- **M1 — stdio client primitive:** `mcp/connect` (stdio) + `initialize` +
  `mcp/tools` + `mcp/call` + `mcp/close`; capability-gated (`PROCESS`); a test
  against the reference filesystem MCP server. *Acceptance:* list + call a real
  MCP tool from Sema.
- **M2 — agent adapter:** `mcp/tools->sema`; a `defagent` that uses an MCP tool
  end-to-end (replayed via cassette in CI). *Acceptance:* an agent completes a
  task using an external MCP tool, deterministically in CI.
- **M3 — native Streamable HTTP transport** (the wire contract above:
  `Mcp-Session-Id`, JSON-or-SSE branching, `MCP-Protocol-Version`) **+
  `:headers`/bring-your-own-token + `:mcp-servers` sugar on `defagent`.**
  Capability-gated (`NETWORK`). *Acceptance:* connect to a remote server with a
  static bearer token (no OAuth yet) against a real Streamable-HTTP server.
- **M4 — native OAuth 2.1 login flow** (the auth architecture above): the auth-store
  trait + engine; config dir + token store (keychain w/ `0600`-file fallback);
  PRM/AS discovery; DCR/CIMD/pre-registered client; PKCE; browser open + loopback
  callback (deterministic→ephemeral port); refresh + `403` re-scope + self-heal;
  `sema mcp login`/`logout` CLI incl. the headless paste fallback. Test the engine
  in CI against `rust-mcp-stack/oauth2-test-server`; verify once live against a real
  OAuth-gated server (Asana/Linear). *Acceptance:* `mcp/connect` to a real
  OAuth-gated server completes the browser flow once, then reconnects silently from
  cached tokens; CI exercises the full flow against the local test IdP.
- **M5 — MCP-call cassette recording** (shared format with LLM cassettes).

## Open questions

- Handle lifecycle: explicit `mcp/close` vs. drop-based cleanup vs. both. Leaning
  both (drop closes; explicit for determinism).
- Reconnect/retry policy for long-lived agent sessions.
- Should `defagent` own connection lifetime, or should the user pass a live
  handle (current sketch)? Passing a handle is more composable; revisit after M2.
- Surface for MCP **resources/prompts** (MCP has more than tools) — defer until
  tools land; tools are 90% of the value.

### Auth-specific open questions

- **Build vs reuse (M0 gate, the big one):** `rmcp` (Option A — spec-complete OAuth,
  large async surface to bridge into single-threaded `block_on`) vs hand-roll +
  `oauth2` (Option B — matches the existing hand-rolled server, recommended).
  reqwest version is *not* a blocker (both on 0.13.2, verified). Decide via the M0
  spikes.
- **Token storage default:** keychain (`keyring-core`) primary with `0600`-file
  fallback from day one, or ship the file first and add keychain later? Keychain is
  the right default but adds per-platform deps + the `spawn_blocking`/headless
  handling. Leaning keychain-first since the fallback is needed regardless.
- **Browser-open capability:** new dedicated capability bit, or fold "launch a
  browser" into the existing `PROCESS` cap? Leaning fold-into-`PROCESS` to avoid
  bitset churn, but it's a distinct authority.
- **`client_id` for Sema itself:** register a Sema OAuth client per provider, host
  a **CIMD** document at a `sema-lang.com` URL (the spec's preferred path — clean if
  we have a web presence, but few commercial AS honor CIMD yet), rely on **DCR**
  where offered, or require the user to bring a pre-registered client_id? Asana/Linear
  likely force per-user pre-registration today, so support all of pre-set / DCR /
  CIMD and pick per-server at runtime.
- **Asana RFC 8252 gap:** Asana's AS reportedly hasn't accepted loopback redirect
  URIs — confirm against the live server (via the mcp-remote oracle) before
  committing to Asana as the M4 acceptance target; Linear or GitHub's hosted MCP may
  be a smoother first real-server test.
- **Headless flow:** is print-URL-and-paste-the-code enough, or add a
  device-authorization grant (only some AS support it) for fully headless boxes?
- **Asana RFC 8252 gap:** Asana's AS reportedly doesn't yet accept loopback
  redirect URIs — confirm before committing to native OAuth against Asana
  specifically; mcp-remote may be the only working path there for now.
- **Headless flow:** is the print-URL-and-paste-code fallback enough, or do we want
  a device-authorization grant (only some servers support it)?
- **Token refresh & multi-process:** if two Sema processes share the token file,
  how do we avoid refresh races / clobbering? (File lock, or accept last-writer.)
