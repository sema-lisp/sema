# Authenticated MCP servers in workflow runs — scoping (future feature)

> **Status (2026-07-09):** the hard blocker is gone — the MCP client with OAuth shipped (`crates/sema-mcp/{client.rs,client_auth.rs,oauth/}`, PR #59). The workflow `:mcp` projection and dashboard auth surface described here are still not started; this plan is now actionable as written.

**Status:** Scoping / future feature (2026-06-24). **Not started.** Depends on work
that does not exist yet (the MCP *client* + its OAuth engine). This doc scopes the
*workflow + dashboard* integration: how a `defworkflow` declares the MCP servers /
tools it needs, how the web UI drives the login flow for the ones that require
auth, and how the resulting auth session is persisted so the run (and later runs)
can use it.

**Companions (read together):**
- `docs/plans/2026-06-21-mcp-client-spike.md` — Sema as an MCP *client* (Layer 1
  `mcp/connect|tools|call|close`, the OAuth 2.1 / PKCE engine, browser+loopback
  login, token store). **This feature is the workflow/dashboard projection of that
  work and is HARD-BLOCKED on it.** (That doc is in-progress; this one only
  references it.)
- `docs/plans/archive/2026-06-21-dynamic-workflows-scoping.md` + `docs/plans/archive/2026-06-23-dynamic-workflows-derisk-spikes.md`
  — the workflow runtime (`defworkflow`/`workflow/run`/`phase`/`checkpoint`,
  the frozen JSONL journal, `.sema/runs/<run-id>/`). **Shipped: Spike 1 + Spike 3.**
- `docs/plans/archive/2026-06-23-workflow-dashboard-scope.md` — `sema workflow view`
  (the read-only viewer; Option-A spike shipped). The auth flow turns the viewer
  from read-only into the run's **control surface** for HITL gates.
- The HITL approval-gate milestone (deferred) in the scoping doc §2.3 — auth is the
  first concrete HITL gate; it should reuse the async yield-signal mechanism.

---

## 1. The problem & the target scenario

Workflows are most valuable when their leaf workers can reach real systems — and
the richest of those are **authenticated remote MCP servers** (Asana, Linear,
hosted GitHub, Slack, …). Today a Sema script that wanted Asana would have to
hand-wire `deftool`s and smuggle a token in via env. We want a workflow to
**declare** "I need the Asana MCP server", and for the runtime + dashboard to:

1. detect, **before any leaf runs**, which declared servers need auth and are not
   yet authorized;
2. **pause** the run at that gate (no compute burned) and surface it in the web UI;
3. let the user complete an **OAuth login flow** from the dashboard (one click →
   browser consent → callback);
4. **persist** the resulting session so the run continues and **later runs reuse
   it** without re-login until it expires;
5. inject the credential into the leaf's `mcp/connect` transparently.

> **Target scenario (the owner's example).** `defworkflow triage` declares it needs
> `asana`. `sema workflow run triage.sema` starts, hits the auth gate (no Asana
> token yet), and prints *"auth required — open `sema workflow view`"*. The user
> opens the dashboard, sees **Asana · not connected · [Connect]**, clicks it, the
> browser pops the Asana OAuth consent, the callback lands, the token is persisted
> to the run/workflow auth store, the gate clears, and the run proceeds — its
> `asana` leaves now authenticated. The next run of `triage` is silent (cached
> token, refreshed as needed).

This is a "make the agentic story reach real SaaS" feature, gated behind the MCP
client landing. It is **not** a correctness fix and must not jump that queue.

---

## 2. Surface syntax — declaring required servers in `defworkflow`

Declarations live in the workflow **meta map** (next to `:budget`/`:permissions`/`:args`),
so the requirement is **static and inspectable before the body runs** — the whole
point of a deterministic orchestrator. A new `:mcp` key maps a local alias to a
server spec (the same spec `mcp/connect` accepts) plus an optional `:auth` hint and
the tools the workflow actually uses (for least-privilege + a verifiable manifest):

```sema
(defworkflow triage
  "Triage new bugs into the Asana board."
  {:args {:repo :string}
   :budget {:max-tokens 250000}
   :mcp {asana {:url   "https://mcp.asana.com/mcp"
                :auth  {:scopes ["default"]}        ; OAuth needed; PKCE login
                :tools ["create_task" "search_tasks"]   ; least-privilege manifest
                :persist :workflow}                 ; where the session is stored (§4)
         fs    {:command "npx" :args ["-y" "@modelcontextprotocol/server-filesystem" "."]}}}

  (phase "Auth")        ; implicit/automatic — the runtime resolves :mcp here (§3)

  (phase "Triage"
    ;; `asana` resolves to a connected, authenticated handle for the run.
    (workflow/foreach (fn (bug) (workflow/agent (:id bug)
                       (fn () (mcp/call asana "create_task" (->task bug)))))
                      (checkpoint :bugs) 4)))
```

Design decisions:
- **`:mcp` is a map alias→spec**, mirroring `mcp/connect`'s spec (so there is one
  spec shape, not two — the §"One canonical request" discipline from `sema-llm`).
- A declared alias (`asana`, `fs`) is **bound in the workflow scope** to a live,
  already-connected MCP handle — the body never calls `mcp/connect` itself, so the
  connect+auth lifecycle is owned by the runtime (and thus journaled + gated).
- `:tools` is an **optional manifest** — used to (a) request least-privilege scopes,
  (b) render the consent screen's "this workflow will be able to …" line, and
  (c) fail fast if a leaf calls an undeclared tool. Omit for "all tools".
- `:auth` present ⇒ the server needs OAuth; absent ⇒ open or token-via-`:headers`
  (bring-your-own, no flow). stdio servers (`fs`) never need a flow.
- `:persist` chooses the session store scope (§4); defaults to `:workflow`.

`defworkflow` stays a macro: `:mcp` is just data in the meta map that
`workflow/run` reads — no new special form.

---

## 3. Lifecycle: declare → preflight → gate → persist → inject

The run gains an implicit **auth-resolution step at the top** (before the first
user `phase`), driven by the runtime, not the body:

```
run.started
  └─ resolve :mcp
       for each declared server:
         • stdio / open / :headers  → connect now, done
         • :auth (OAuth)            → look up a persisted session (§4)
              ├─ valid token   → connect (silent), done
              ├─ expired+refresh → refresh, persist, connect, done
              └─ none/needs-consent → EMIT auth.required, PAUSE the run (gate)
  └─ (gate clears when every required server is authorized) → run the phases
```

**The gate is a HITL pause, not a busy-wait.** It reuses the async yield-signal
mechanism (`sema-core/src/async_signal.rs`; the same one `AwaitIo` uses) so the run
**parks with zero compute** until the credential arrives — exactly the deferred
"HITL approval gate" milestone, with auth as its first instance. Two ways the gate
can clear:

- **Headless precursor (simplest, ship first):** the run does *not* block; it exits
  with a distinct status `{:status :needs-auth :servers [asana]}` and a message to
  authenticate (via the dashboard or a `sema mcp login asana` CLI), then **re-run**
  — now the persisted token is found and the run proceeds. No live gate; auth is a
  separate step. This needs only the token store + a login entry point.
- **Live gate (the full feature):** the run parks at the gate; the dashboard's
  auth flow (§5) writes the session to the store and signals the parked run, which
  wakes and continues in the same process. This needs the yield-gate + the
  dashboard↔runtime channel.

Recommend shipping the **headless precursor first** (it delivers the scenario with
far less machinery) and the live gate as a follow-on.

---

## 4. Persistence — where the auth session lives

The MCP-client doc's default token store is the OS **keyring** (`keyring-core`).
For workflows the owner specifically wants the session persisted "to the workflow
run or in the folder", so this feature adds **scoped file-backed stores** alongside
the keyring, chosen per-server via `:persist`:

| `:persist` | Location | Lifetime / sharing | Use when |
|---|---|---|---|
| `:keyring` | OS keychain (per user) | shared across all runs + workflows | the default for a dev machine; most secure at rest |
| `:workflow` *(default here)* | `.sema/auth/<workflow-name>/<server>.json` | reused by every run of THIS workflow | the owner's "persist to the workflow" ask — re-auth once per workflow, not per run |
| `:run` | `.sema/runs/<run-id>/auth/<server>.json` | this run only | ephemeral / one-off / CI with a short-lived token |
| `:none` | in-memory | this process only | never touches disk |

**Security (non-negotiable for file-backed stores):**
- The store holds **refresh + access tokens** — secrets. Files are written `0600`,
  and `.sema/` is already git-ignored (added with the workflow runtime); add an
  explicit `.sema/auth/` note + a guard that refuses to write a token under a
  directory that is not git-ignored.
- **Encrypt at rest** by default: wrap the token blob with a key from the OS
  keyring (keyring stores the *encryption key*, the file stores the *ciphertext*),
  so a leaked `.sema/auth/*.json` is useless without the keychain. `:run`/`:workflow`
  thus mean "where the ciphertext lives", not "plaintext on disk". `:none`/CI can
  opt into env-var key material.
- **Redaction everywhere:** tokens never enter the journal, `result.json`,
  `metadata.json`, OTel spans, or the dashboard payloads. The journal records only
  `auth.required`/`auth.granted` with a server alias + scope list + expiry — never
  the token. (Mirrors the LLM accounting/redaction discipline.)
- A token file is **side-state, never replayed** (see §7).

`:workflow` is the default because it matches the owner's scenario (auth once per
workflow, reuse across runs) while keeping the secret out of any single ephemeral
run dir.

---

## 5. The web-UI authentication flow (dashboard)

This is what turns `sema workflow view` from read-only into the run's control
surface. New, **write** endpoints on the viewer server (so the no-auth/loopback
security model must be revisited — see §8):

- `GET  /api/run/:id/auth` → the auth manifest + live status per declared server:
  `[{alias:"asana", needs_auth:true, status:"needs-consent"|"authorized"|"expired", scopes:[…], tools:[…]}]` (derived from the new journal events + the token store).
- `POST /api/auth/:server/start` → begins the OAuth flow for that server: the
  server-side MCP-client OAuth engine does discovery → DCR/PKCE → builds the
  authorize URL, opens the browser (`open` crate) and runs the **loopback callback**
  (the MCP-client doc's `127.0.0.1:0` + `/callback` → `oneshot`), exchanges the
  code, persists the session (§4), and **signals the parked run** (live gate) or
  just stores it (headless precursor). Returns `{status:"authorized"|"failed", …}`.
- `POST /api/auth/:server/forget` → delete the stored session (re-consent next time).

**UX (on the variant-5b brand, terminal-quiet):** when a run is at the auth gate,
the run header shows a `needs-auth` pill, and a compact **Auth panel** lists each
required server as a row: `asana · not connected · [Connect]` / `· authorized ·
expires 13:40 · [Forget]`. `[Connect]` → spinner → browser consent → row flips to
`authorized` and the run's `running` resumes. No charts, no chrome — a row per
server, exactly like an agent row. The consent screen's scope/tools come from the
`:tools` manifest so the user sees *what the workflow can do* before granting.

A non-browser/CI path: `sema mcp login asana` (or `sema workflow run … --auth asana`)
does the same flow headlessly where a browser can pop, or accepts a device-code /
pre-issued token.

---

## 6. Journal event vocabulary additions

The frozen ~8-event vocab (append-only policy) gains auth events (all
secret-redacted):

- `auth.required` `{seq, ts, server, scopes, tools, persist}` — emitted when a
  declared server can't be satisfied from the store; the gate opens here.
- `auth.granted`  `{seq, ts, server, scopes, expires_at, source}` — `source` ∈
  `cached | refreshed | consented`; the gate (for that server) closes here.
- `auth.failed`   `{seq, ts, server, reason}` — consent refused / discovery failed /
  callback timed out; the run ends `{:status :failed :reason :auth}`.

These are additive `WorkflowEvent` variants (`crates/sema-workflow/src/event.rs`);
old goldens stay valid. The dashboard's tree renders them as a dim auth line under
the implicit "Auth" phase, and they back the `/api/run/:id/auth` status.

---

## 7. Resume / replay interaction

- **Tokens are side-state, not part of the deterministic skeleton.** Resume
  (Spike 4) re-runs the workflow code from the top; the auth-resolution step is
  re-evaluated and finds the persisted (possibly refreshed) session — it does **not**
  read a token out of the journal. So a journal is shareable/replayable without
  leaking credentials, and a resume after token expiry simply re-gates.
- **Conservative-resume (Flue contract):** an `auth.required` with no matching
  `auth.granted` means the gate was never cleared → on resume the run re-gates,
  never assumes authorization.
- **Cassette/CI:** a cassette-replayed run uses no live MCP server, so the auth gate
  is **bypassed in replay mode** (the recorded tape stands in for the server's
  responses) — the offline-CI oracle never needs real credentials.

---

## 8. Security model (the load-bearing part)

- **The dashboard gains write/auth endpoints**, so the notebook-style "loopback +
  no auth" model is **no longer sufficient on its own** for the auth routes: a
  local process could POST `/api/auth/asana/start` and trigger a consent. Mitigations
  to decide: a per-session CSRF/launch token minted by `sema workflow view` and
  required on POST; binding strictly to loopback; and the OAuth `state` + PKCE
  already pinning the callback. **Do not ship the write endpoints without this.**
- Encrypt-at-rest + `0600` + git-ignore guard (§4). Never log/journal/echo tokens.
- Least privilege: request only the `:tools`-implied scopes; show them at consent.
- `:run`/`:workflow` token files are deleted by `forget`; document that abandoning a
  run does not auto-delete `:workflow` tokens (they are meant to persist).

---

## 9. Dependencies & sequencing

```
[MCP client Layer 1 + OAuth engine + token store]   ← HARD blocker (separate plan)
        │
        ├─► (a) :mcp declaration in defworkflow meta + runtime auth-resolution step
        │        + the headless precursor ({:status :needs-auth}, `sema mcp login`)
        │
        ├─► (b) scoped file-backed token stores (:workflow / :run) + encryption
        │
        ├─► (c) journal auth.* events + dashboard read-only auth status panel
        │
        └─► (d) dashboard WRITE auth endpoints + CSRF + the LIVE HITL gate
                 (reuses the async yield mechanism) — the full one-click flow
```

Build order: the MCP client first (its own plan), then (a)+(b) for the headless
scenario, then (c) for visibility, then (d) for the polished one-click web flow.
(d) also wants the dashboard's **Option B** server (the read-only Option-A spike
has no write path) — so it is naturally sequenced after the SQLite/live-tail
dashboard upgrade.

---

## 10. Open questions

1. **Gate granularity:** auth all declared servers up-front (simpler, one gate) vs
   lazily at first use of each (less waiting, but a gate mid-run)? Lean up-front.
2. **Multi-user / shared runs:** if the dashboard is ever bound non-loopback, whose
   credentials does a run use? Probably out of scope — keep it single-operator.
3. **Token refresh during a long run:** refresh transparently in the
   auth-resolution layer; does a mid-run expiry re-open the gate or just refresh?
   (Refresh silently; only re-gate if refresh fails.)
4. **`:persist :workflow` keyed by name vs content:** if two checkouts define
   different `triage` workflows, the name collides. Key by workflow name + a hash
   of the `:mcp` spec?
5. **Encryption key bootstrapping on headless CI** (no keyring): env-var key, or
   accept plaintext `:run` tokens with a loud warning?
6. **Does `:mcp` belong in `metadata.json`** (so the dashboard knows requirements
   before the run starts)? Yes — record the redacted manifest there.

---

## 11. Non-goals (for this feature)

- Not building the MCP client or the OAuth engine here — that is its own plan and a
  hard dependency.
- No credential *sharing* across machines / no secret-manager backends beyond the
  OS keyring (vault/cloud KMS is a later, separate concern).
- No general "secrets management" for arbitrary `:env`/API keys — this is
  specifically MCP-server auth sessions. (A broader workflow-secrets feature could
  generalize the store later.)
- No multi-tenant / hosted dashboard auth — single local operator only.
