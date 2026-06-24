# Workflow Dashboard + SQLite Projection — Scoping

> ## ⚑ OPTION-A SPIKE SHIPPED 2026-06-24 (branch `feat/dynamic-workflows-spike1`)
> The visible viewer is built: `sema workflow view --run-dir <dir> [--host --port]`.
> - `crates/sema/src/workflow_view.rs` — a minimal loopback tokio HTTP/1.1 static server
>   (NO new axum dep; 4 read-only routes: `/`, `/alpine.min.js`, `/api/runs`,
>   `/api/run/<id>/events.jsonl`). Loopback + no-auth, documented (same model as the notebook).
>   Path-traversal in the run id is rejected. Unit-tested routing/safety.
> - `crates/sema/src/workflow_view/index.html` — the AlpineJS tree viewer (vendored alpine, same
>   as the notebook), on the Sema brand palette: run header + status pill + rollup, phase groups,
>   agent rows merging started→result with status glyph + duration, tool twigs, checkpoint/budget
>   lines, expand-to-see output, follow-live polling while a run is `running`. Honors §1.2
>   anti-dashboard-isms. Smoke-verified against the live content-pipeline journal
>   (`crates/sema/tests/fixtures/workflow/content-live.events.jsonl`).
> - This is **Option A** (§2.2 — static client-side parse of `events.jsonl`). **Option B** (the
>   SQLite projection + idempotent replay-to-rows ingester + server-side `seq > N` live-tail cursor,
>   §3–§5) is the documented upgrade for many/large/long runs — **still to build**. Browser-visual
>   fidelity vs `variant-5b` is pending a human look.

**Status:** scoping · Option A spike shipped 2026-06-24 · Option B (SQLite) pending · **Design direction LOCKED 2026-06-23.**
**Depends on:** Spike 1 (sequential runtime + *frozen* JSONL journal) — see
`docs/plans/2026-06-23-dynamic-workflows-derisk-spikes.md` §"Spike 1". This work
**must not start until the ~8-event vocabulary is frozen** (scoping
`2026-06-21-dynamic-workflows-scoping.md` §3.5, open question #5).

**Chosen prototype (the visual contract):** `docs/plans/prototypes/variant-5b-sema-tui.html`
— a **modernized, webified version of the Claude Code `/workflows` display**, on the Sema
brand (warm `#131110` IDE canvas, gold used only for the running agent/active selection,
JetBrains Mono, the `(sema)` logotype). This is the locked basis. Its layout: a pinned run
header (`(sema) Workflow — <name>`, centered status pill + elapsed, right-aligned
`N agents · NN.Nk tok · $cost` rollup) over a three-pane body — **left** a phase ledger with
`done/total` counts, **center** a dense aligned agent table (`agent · model · tok · tools ·
dur`) with an inline drill-in (Prompt / Activity / Outcome), **right** the raw
`events.jsonl` stream (each row clickable → jumps to its agent/phase). Keyboard nav
(Tab/↑/↓/Esc). Every value shown maps to a real producible event — see §3.

**Explored alternatives** (kept in `docs/plans/prototypes/` for reference, not chosen):
`variant-2c-sema-devtools` (split-pane IDE inspector), `variant-3c-sema-runlog` (CI-run-log
vertical scroll). The earlier generic-styled set (`variant-1`, `4`, and the pre-brand
`2b`/`3b`/`5`/`0`) were rejected for not following the Sema brand.

> One-line framing: this is the *structured upgrade of `tail -f events.jsonl | jq`*.
> The journal already works on day one; the dashboard is a read-only projection
> that makes a running workflow **look like watching a Workflow run in Claude
> Code** — an indented, self-updating tree, not a BI dashboard.

---

## 1. What we're emulating (and what we're not)

### 1.1 The thing we're copying

When a Workflow runs in Claude Code you watch a **live, indented, self-updating
tree**: a pinned run header, phases as group headers, agents fanning out as
indented child rows that spin while they work and mutate in place when they
finish, tool calls as deeper twigs, and a low-chrome narration stream threaded
through it all. Concurrency is visible as *several rows spinning at once*. When
the run ends, the tree freezes into a static summary you can re-expand.

That is the entire aesthetic target. Concretely, we reproduce these behaviours:

1. **Run header, pinned at top.** Workflow name + a live status pill
   (`running` / `success` / `failed` / `interrupted`), a wall-clock elapsed timer
   that ticks while running, and a compact right-aligned rollup —
   `NN agents · 142k tok · $0.38` — i.e. the `run.started → run.ended` envelope
   made visible.
2. **Phases as labelled groups.** Each `phase` is a collapsible group-header row:
   a left status glyph (spinner while open, check on success, `x` on failed), the
   phase label, and a dim right-aligned duration that fills in on `phase.ended`
   (`dur_ms`; `0` under the fixed-ts test seam). Phases stack in seq order; the
   open phase is the highlighted/animated "live" one; ended phases go dim.
3. **Agents fanning out under a phase.** Agents are indented child rows
   (one nesting level, `├─` connectors) with a per-agent status glyph, a
   name/label (e.g. `auditor (app/Http/X.php)`), and a right-aligned per-agent
   token meter that climbs on tool-call/result. A concurrent `foreach` fan-out
   renders as **sibling rows spinning simultaneously** — N at once is the visual
   proof of bounded `:max-concurrent` parallelism. An agent row **mutates in place**
   from `started → result`; it never appends a second row.
4. **Tool calls nested under agents.** `agent.tool_call` renders as a deeper faint
   twig (`├─ tool: file/read app/X.php`) under its agent.
5. **Narrator log.** A chronological, low-chrome stream of one-line entries
   threaded *into* the tree at their seq position (interleaved with phase/agent
   rows), reading like Claude Code's narration. Checkpoints surface as quiet
   `saved :findings (3 items)` lines; budget events as muted `budget 142k/250k (57%)`
   lines.
6. **Expand-to-see-I/O.** The tree nests `run → phase → agent → tool_call`. Any
   agent / checkpoint / tool row expands to an inline monospace disclosure showing
   input/output (agent output is `string` / `output_digest` only — see §3),
   collapsed by default, exactly like clicking a step in Claude Code.
7. **Follow-live.** While running, new `events.jsonl` rows stream in (`tail -f`
   style) with one spinner on the active leaf and auto-scroll. On `run.ended` the
   tree freezes into a final static summary collapsed to phase level, re-expandable.

**Aesthetic:** terminal-quiet monospace, glyphs, indentation, dim metadata.

### 1.2 Explicit non-goals (anti-dashboard-isms)

This is a live log tree, **not** a dashboard. The following are out of scope and
should be actively rejected in review:

- **No KPI/stat tiles** row across the top ("Total Tokens" / "Avg Latency" /
  "Success Rate" big-number cards).
- **No charts:** no donut/pie status chart, token-usage bar chart, cost-over-time
  line graph, or sparklines.
- **No gauge/speedometer for budget.** Budget is a plain `142k/250k (57%)` text
  line, at most a *thin inline underline bar* — never a radial gauge.
- **No timeline / Gantt swimlane** of agent spans. Concurrency is shown by
  simultaneous spinners in the tree, not parallel bars on a time axis.
- **No DAG / node-graph canvas** with draggable boxes and edges. This is a live
  log tree, not a LangGraph-Studio graph editor.
- **No heatmaps, calendar views, geographic visualizations.**
- **No multi-run analytics, leaderboards, or trend comparisons** in this
  single-run view.
- **No gradient hero, glassmorphism, bento grid, or rounded drop-shadowed cards.**
  Keep it terminal-quiet.
- **No ambient polling/refresh spinner chrome** — exactly one spinner per
  genuinely-running leaf.
- **No fabricated data the journal doesn't emit** — no per-agent latency
  percentiles, temperature charts, or tokens/sec graphs. Render only the frozen
  ~8-event fields; agent output is a string/digest, not a rich diff view.

### 1.3 It can be its own thing

This need **not** live inside `sema-notebook`. The notebook is a cell-evaluation
surface with a shared interpreter; this is a read-only tail-and-render viewer over
a file. Coupling them would drag in the notebook's evaluation engine and security
model for no benefit. The viewer is a small standalone artifact (one HTML file,
or a thin `sema workflow view` server — see §2), reusing only the shipped SQLite
stdlib layer.

---

## 2. Architecture

### 2.1 Source of truth vs projection

```
.sema/runs/<run-id>/            ← the stable public contract (scoping §3.5)
  events.jsonl                  ← SYSTEM OF RECORD (append-only, flush-per-event)
  args.json  metadata.json  result.json  checkpoints/  artifacts/

        │  replay-to-rows ingest (idempotent, keyed on (run_id, seq))
        ▼
  workflow.db (SQLite)          ← READ-ONLY PROJECTION (disposable, rebuildable)
        │
        ▼
  the viewer (HTML tree)        ← renders queries from the projection
```

- `events.jsonl` is the **only** authority. It is append-only, written
  flush-per-event by a verbatim copy of `JsonlFileExporter` (`OpenOptions`
  append+create, `BufWriter`, one `serde_json` line + `\n`, flush each event;
  see derisk Spike 1). The run directory is the documented public contract,
  treated like the `.semac` bytecode format.
- **SQLite is a derived view, never the system of record.** Nothing writes to it
  except the ingester replaying the journal. The workflow runtime never reads it
  to make decisions (resume reads `events.jsonl` + `checkpoints/` on disk). It can
  lag, be rebuilt, or be entirely absent without affecting correctness. Delete it
  and replay from byte 0 → byte-identical rows. A "schema change" is therefore
  *drop + re-replay*, never a data migration.
- Run-dir location is **project-local `./.sema/runs/<run-id>/`** (cwd-relative,
  git-ignorable), not user-global `~/.sema` — fixed in Spike 1.

### 2.2 Tiny built-in server vs static file — **recommendation**

Two viable shapes, both grounded in shipped Sema infrastructure:

**Option A — static HTML reading `events.jsonl` directly.** A single self-contained
`.html` file `fetch()`es `events.jsonl`, parses lines client-side, and renders the
tree. No server, no SQLite. This is literally `tail -f | jq` in a browser tab.
- ✅ Zero moving parts; the prototype already proves the rendering.
- ❌ Browsers can't `tail` a growing file cheaply (re-`fetch` re-downloads the whole
  file each poll); no shared indexed queries; needs a static file server anyway to
  avoid `file://` CORS/fetch limits; reparses the full journal on every tick.
  Fine for the **prototype and for small/finished runs**, weak for live long runs.

**Option B — `sema workflow view` (recommended).** A tiny built-in HTTP server,
modelled directly on the **notebook server** (`crates/sema-notebook/src/server.rs`):
- **axum**, **loopback-only by default** (`DEFAULT_HOST = "127.0.0.1"`), single
  binary, no auth — same *trusted-local developer tool* security model the notebook
  already documents. (Binding non-loopback is the operator's responsibility, same
  caveat as the notebook.)
- **UI assets embedded at compile time via `include_str!`**, exactly like the
  notebook (`crates/sema-notebook/src/ui.rs`: `include_str!("ui/index.html")` etc.)
  — deployment stays a single binary.
- The server runs the **ingester loop** (tail `events.jsonl` → project into SQLite,
  §5) and exposes a handful of JSON endpoints backed by the §4 queries:
  `GET /api/runs`, `GET /api/run/:id`, `GET /api/run/:id/tree`,
  `GET /api/run/:id/events?since=:seq` (live-tail cursor),
  `GET /api/agent/:run/:agent` (step detail / tool calls).
- The SQLite work is done **entirely in Sema** via the shipped layer
  (`crates/sema-stdlib/src/sqlite.rs`: `db/open`, `db/exec`, `db/exec-batch`,
  `db/query`; rusqlite 0.40 bundled — **no new Rust dependency**). `db/open`
  already runs `PRAGMA journal_mode=WAL`, giving single-writer / many-reader
  concurrency for free.

**Recommendation: Option B (`sema workflow view`), with the static file (Option A)
kept as the prototype and the "view a finished run with nothing installed" escape
hatch.** Rationale: the notebook server is the proven template (loopback axum +
`include_str!` single-binary + no-auth local tool), the live-tail story needs a
server-side cursor (`seq > N`) so the UI never re-downloads settled rows, and
SQLite's WAL mode lets the ingester append while the viewer reads. We get all of
this with zero new Rust — only Sema code over the shipped SQLite stdlib plus an
axum router cloned from the notebook. (Consistent with scoping open-question #4:
*one binary, no daemon for MVP*.)

---

## 3. SQLite schema + idempotent replay-to-rows ingest

> **Ground-truth invariant:** SQLite is a *derived view* over
> `.sema/runs/<run-id>/events.jsonl`. Every row is reproducible by replaying the
> journal; the DB can be deleted and rebuilt at any time. **No triggers, no app
> writes except the ingester.** Built from Sema with the shipped sqlite layer
> (`db/open` / `db/exec` / `db/exec-batch` / `db/query`,
> `crates/sema-stdlib/src/sqlite.rs:39+`, rusqlite 0.40 bundled). Run once at open:
> `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;` (via `db/exec-batch`, which
> runs static SQL verbatim) so the viewer can read while the ingester appends.

### 3.1 Tables

```sql
-- raw event log: a 1:1 landing table for every JSONL line. The idempotency spine
-- and the audit/rebuild source. (run_id, seq) is the natural key every event
-- carries (each variant has seq:u64 + ts:String). 'event' is the serde tag field
-- (serde(tag="event")).
CREATE TABLE IF NOT EXISTS events (
  run_id      TEXT    NOT NULL,
  seq         INTEGER NOT NULL,        -- monotonic per run; the dedup key
  event       TEXT    NOT NULL,        -- frozen tag: run.started | phase.started | ...
  ts          TEXT    NOT NULL,        -- ISO-ish string straight from the journal
  phase       TEXT,                    -- denormalized for fast filtering (NULL for run.*)
  agent_id    TEXT,                    -- present on agent.* events
  payload     TEXT    NOT NULL,        -- the verbatim JSON line (lossless escape hatch)
  ingested_at TEXT    NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (run_id, seq)            -- INSERT OR IGNORE on this = idempotent replay
);

-- one row per run. Populated/updated from run.started + run.ended. Mutable columns
-- (status/ended_*) start NULL and fill when run.ended arrives, so a live run is
-- simply status IS NULL / ended_ts IS NULL.
CREATE TABLE IF NOT EXISTS runs (
  run_id       TEXT PRIMARY KEY,
  workflow     TEXT,                   -- workflow name (run.started / metadata.json)
  code_version TEXT,                   -- for replay identity
  started_ts   TEXT,
  started_seq  INTEGER,                -- seq of run.started (origin marker)
  ended_ts     TEXT,                   -- NULL while live
  status       TEXT,                   -- success | failed | budget-exceeded | NULL(live)
  args_json    TEXT,                   -- the --args envelope, if carried on run.started
  last_seq     INTEGER NOT NULL DEFAULT 0  -- high-water mark for live tailing
);

-- one row per phase invocation. Opened by phase.started, closed by phase.ended.
-- Keyed (run_id, start_seq) because a workflow may enter the same phase label
-- twice (e.g. retry) — label is NOT unique, the opening seq is.
CREATE TABLE IF NOT EXISTS phases (
  run_id     TEXT    NOT NULL,
  start_seq  INTEGER NOT NULL,         -- seq of this phase.started
  phase      TEXT    NOT NULL,         -- label
  parent_seq INTEGER,                  -- nesting: enclosing phase's start_seq (tree)
  started_ts TEXT,
  ended_ts   TEXT,                     -- NULL while running
  status     TEXT,                     -- success | failed | NULL(running)
  dur_ms     INTEGER,                  -- from phase.ended (0 under the fixed-ts seam)
  PRIMARY KEY (run_id, start_seq),
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

-- one row per agent invocation (a 'step'). Opened by agent.started, closed by
-- agent.result. agent_id is the correlation key the journal carries on both;
-- (run_id, agent_id) is the identity. phase_seq ties it into the phase tree.
-- output kept opaque (string/digest) on purpose so typed findings can be added
-- later without a schema break.
CREATE TABLE IF NOT EXISTS agents (
  run_id      TEXT    NOT NULL,
  agent_id    TEXT    NOT NULL,
  phase_seq   INTEGER,                 -- the phases.start_seq this agent ran under
  agent_name  TEXT,                    -- subagent / role name
  model       TEXT,
  start_seq   INTEGER,                 -- seq of agent.started
  result_seq  INTEGER,                 -- seq of agent.result (NULL while running)
  started_ts  TEXT,
  ended_ts    TEXT,
  status      TEXT,                    -- ok | failed | NULL(running)
  output      TEXT,                    -- opaque string / digest, NOT typed yet
  dur_ms      INTEGER,
  PRIMARY KEY (run_id, agent_id),
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

-- one row per agent.tool_call event. Many per agent. Append-only; keyed on the
-- originating event seq so re-ingest is idempotent.
CREATE TABLE IF NOT EXISTS tool_calls (
  run_id    TEXT    NOT NULL,
  seq       INTEGER NOT NULL,          -- seq of the agent.tool_call event
  agent_id  TEXT    NOT NULL,
  tool_name TEXT,
  args_json TEXT,                      -- tool input as JSON
  result_digest TEXT,                  -- opaque (string/digest), mirrors output policy
  ts        TEXT,
  PRIMARY KEY (run_id, seq),
  FOREIGN KEY (run_id, agent_id) REFERENCES agents(run_id, agent_id)
);

-- one row per checkpoint event (the memoization hook: (checkpoint :k v)). Holds
-- the content key + a value digest, NOT the full value (the real value lives in
-- checkpoints/ on disk). Lets the viewer show what was memoized and which steps a
-- resume would short-circuit.
CREATE TABLE IF NOT EXISTS checkpoints (
  run_id       TEXT    NOT NULL,
  seq          INTEGER NOT NULL,
  phase_seq    INTEGER,
  key          TEXT    NOT NULL,       -- the checkpoint keyword name
  content_key  TEXT,                   -- stable hash of inputs+code version (resume key)
  value_digest TEXT,                   -- short digest of the recorded value
  ts           TEXT,
  PRIMARY KEY (run_id, seq),
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

-- usage / budget rollup. Sourced from the 'budget' event (and any usage fields on
-- agent.result). One row per budget/usage event so it is append-only and
-- idempotent on seq; aggregate at query time. Per-event (not a single mutable
-- counter) preserves the accounting invariant: a cache hit reports zero usage, so
-- summing events never double-charges (CLAUDE.md LLM accounting rule).
CREATE TABLE IF NOT EXISTS usage (
  run_id        TEXT    NOT NULL,
  seq           INTEGER NOT NULL,      -- seq of the budget/agent.result event
  agent_id      TEXT,                  -- NULL for run-level budget snapshots
  phase_seq     INTEGER,
  input_tokens  INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cost_usd      REAL    NOT NULL DEFAULT 0,
  budget_limit  INTEGER,               -- :max-tokens snapshot if carried
  ts            TEXT,
  PRIMARY KEY (run_id, seq),
  FOREIGN KEY (run_id) REFERENCES runs(run_id)
);

-- indexes the viewer actually needs (everything else covered by the PKs).
CREATE INDEX IF NOT EXISTS ix_events_run_seq   ON events(run_id, seq);
CREATE INDEX IF NOT EXISTS ix_phases_parent     ON phases(run_id, parent_seq);
CREATE INDEX IF NOT EXISTS ix_agents_phase       ON agents(run_id, phase_seq);
CREATE INDEX IF NOT EXISTS ix_tool_calls_agent   ON tool_calls(run_id, agent_id);
CREATE INDEX IF NOT EXISTS ix_usage_run          ON usage(run_id);

-- stored cursor so the live ingester knows where it left off per run/file.
CREATE TABLE IF NOT EXISTS ingest_cursor (
  run_id       TEXT PRIMARY KEY,
  byte_offset  INTEGER NOT NULL DEFAULT 0,  -- bytes consumed from events.jsonl
  last_seq     INTEGER NOT NULL DEFAULT 0,
  updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### 3.2 Replay-to-rows ingest (idempotent, keyed on `(run_id, seq)`)

The ingester reads new JSONL lines; for each line it runs **one transaction**
(`db/exec-batch "BEGIN … COMMIT"`) that (1) lands the raw line, then (2) projects
it into the typed table. Idempotency is **structural, not logical**: every
projecting statement is `INSERT OR IGNORE` on a `(run_id, seq)`-rooted primary
key, or an `UPDATE` guarded so re-applying is a no-op. Re-ingesting the whole file,
or overlapping ranges after a crash, converges to the same rows.

**Step 0 (always):**
`INSERT OR IGNORE INTO events(run_id, seq, event, ts, phase, agent_id, payload) VALUES (…)`.
If this IGNOREs (row already present), the line was projected in a prior run →
**skip the rest of the projection for that line** (check `changes()`/rowcount;
`0` means already-seen). This single guard makes the whole pipeline idempotent
without per-table bookkeeping.

**Per-event projection (dispatch on `event`):**

- `run.started` → `INSERT OR IGNORE INTO runs(run_id, workflow, code_version, started_ts, started_seq, args_json, last_seq)`.
- `phase.started` → `INSERT OR IGNORE INTO phases(run_id, start_seq=seq, phase, parent_seq, started_ts)`. `parent_seq` = the `start_seq` of the currently-open (un-ended) phase for this run with the greatest `start_seq < this seq` (a one-row subquery), giving the nesting tree.
- `phase.ended` → `UPDATE phases SET ended_ts, status, dur_ms WHERE run_id=? AND start_seq = (most recent open phase of that label: greatest start_seq with matching phase AND ended_ts IS NULL)`. Idempotent because it only fills NULLs (`AND ended_ts IS NULL`).
- `agent.started` → `INSERT OR IGNORE INTO agents(run_id, agent_id, phase_seq=current open phase, agent_name, model, start_seq=seq, started_ts, status=NULL)`.
- `agent.result` → `UPDATE agents SET result_seq=seq, ended_ts, status, output(digest), dur_ms WHERE run_id=? AND agent_id=? AND result_seq IS NULL`. Also `INSERT OR IGNORE INTO usage(… from any usage fields on the result, key=seq)`.
- `agent.tool_call` → `INSERT OR IGNORE INTO tool_calls(run_id, seq, agent_id, tool_name, args_json, result_digest, ts)`.
- `checkpoint` → `INSERT OR IGNORE INTO checkpoints(run_id, seq, phase_seq, key, content_key, value_digest, ts)`.
- `budget` → `INSERT OR IGNORE INTO usage(run_id, seq, agent_id, phase_seq, input_tokens, output_tokens, cost_usd, budget_limit, ts)`. Per-event row, never a mutated counter, so summation can't double-charge (matches the cache-hit-reports-zero-usage accounting invariant).
- `run.ended` → `UPDATE runs SET ended_ts, status WHERE run_id=? AND ended_ts IS NULL`.

**Always, last:** `UPDATE runs SET last_seq = MAX(last_seq, ?seq)` and bump
`ingest_cursor(byte_offset, last_seq)`.

Because `seq` is monotonic per run and `serde(tag="event")` fixes the discriminator
field name `event`, the parser is a single match on that field. The dispatcher is
implemented in Sema: read lines, `json/decode` each, `(:event ev)`, route through a
`cond`, call `db/exec` with **bound params (never string-interpolated)**. A full
rebuild is just: DELETE the run's rows (or DROP + recreate the DB) and replay from
`byte_offset 0` — the same code path, same result.

---

## 4. The viewer + the queries behind each component

The viewer renders the prototype (`docs/plans/prototypes/workflow-dashboard.html`):
a single vertical monospace column, generous left indentation, tree connectors
(`│ ├─ └─`), `[status glyph][flexible label][right-aligned dim metric]` rows. The
**tree is the dominant element (~85% of screen)**.

### 4.1 Components → backing queries

| Component | What it renders | Query |
|---|---|---|
| **RunHeaderBar** | name, status pill, ticking elapsed timer (`start_ts → now/end_ts`), agent count, tokens, cost; flips running→final on `run.ended` | *Run summary* (Q1) |
| **RunMetaStrip** | dim sub-line: `run_id`, `started_at`, code-version short hash, budget cap text | Q1 (`code_version`, `args_json`) + latest `usage.budget_limit` |
| **WorkflowTree** | root scrollable container; renders all events in seq order as a nested indented list; owns expand/collapse + follow-live scroll | *Live tail* (Q6) for ordering + Q2/Q3 for structure |
| **PhaseGroupRow** | per `phase.started`: glyph (spinner→check/x), label, right-aligned `dur_ms`; collapsible; highlighted while open, dim when ended | *Phase tree* (Q2) |
| **AgentRow** | per `agent.started`, mutated in place by `agent.result`: glyph, name/label + target, right-aligned per-agent TokenMeter; expandable; siblings spinning = fan-out concurrency | *Agents within a phase* (Q3) |
| **ToolCallTwig** | per `agent.tool_call`: deepest indent, faint, `tool: <name> <short-args>`; expandable to full args | *Tool-call timeline* (Q4) |
| **NarratorLine** | interleaved one-line entries derived from phase/agent transitions, rendered inline at their seq position | derived from Q6 / Q2 / Q3 |
| **CheckpointLine** | quiet `saved :key (summary/count)`; expandable to value digest | *Resume preview* (Q8) |
| **BudgetLine** | muted `used/cap (pct)`; warning-toned if `tripped` | Q1 totals + latest `usage` row (`budget_limit`) |
| **IoDisclosurePanel** | inline monospace input/output revealed under an expanded Agent/Tool/Checkpoint row | Q4 (tool args/digest) / `agents.output` / `checkpoints.value_digest`; deep value via `checkpoints/` file |
| **StatusGlyph** | shared spinner/check/x/interrupted glyph (terminal style) | `COALESCE(status,'running')` everywhere |
| **TokenMeter** | compact inline `Nk tok` text (text + at most a thin underline bar) | per-agent tokens in Q3 |
| **FollowLiveController** | tail-`f` scroll + bottom control strip (follow toggle, filter box, expand/collapse-to-phases) | drives Q6 polling |
| **FinalEnvelopeBar** | on `run.ended`: inline `{:status}` summary (status, phase/agent counts, tokens, cost, total duration) | Q1 (with `ended_ts` set) |

### 4.2 The queries

```sql
-- Q1 Run summary (header card): one run, live-or-done, with rolled-up totals.
SELECT r.run_id, r.workflow, r.code_version, r.started_ts, r.ended_ts,
       COALESCE(r.status, 'running') AS status,
       (SELECT COUNT(*) FROM phases p WHERE p.run_id = r.run_id) AS phase_count,
       (SELECT COUNT(*) FROM agents a WHERE a.run_id = r.run_id) AS agent_count,
       (SELECT COUNT(*) FROM agents a WHERE a.run_id = r.run_id AND a.status IS NULL) AS agents_running,
       (SELECT IFNULL(SUM(u.input_tokens + u.output_tokens),0) FROM usage u WHERE u.run_id = r.run_id) AS total_tokens,
       (SELECT IFNULL(SUM(u.cost_usd),0) FROM usage u WHERE u.run_id = r.run_id) AS total_cost_usd,
       r.last_seq
FROM runs r
WHERE r.run_id = ?1;

-- Q2 Phase tree: ordered, with depth via recursive CTE over parent_seq.
WITH RECURSIVE tree(run_id, start_seq, phase, parent_seq, status, dur_ms, depth, ord) AS (
  SELECT run_id, start_seq, phase, parent_seq, status, dur_ms, 0,
         printf('%012d', start_seq)
  FROM phases WHERE run_id = ?1 AND parent_seq IS NULL
  UNION ALL
  SELECT p.run_id, p.start_seq, p.phase, p.parent_seq, p.status, p.dur_ms,
         t.depth + 1, t.ord || '.' || printf('%012d', p.start_seq)
  FROM phases p JOIN tree t ON p.parent_seq = t.start_seq AND p.run_id = t.run_id
)
SELECT start_seq, phase, depth, COALESCE(status,'running') AS status, dur_ms
FROM tree ORDER BY ord;

-- Q3 Agents within a phase (expand a phase node): steps + live status + per-step cost.
SELECT a.agent_id, a.agent_name, a.model,
       COALESCE(a.status,'running') AS status, a.dur_ms,
       (SELECT COUNT(*) FROM tool_calls tc WHERE tc.run_id = a.run_id AND tc.agent_id = a.agent_id) AS tool_calls,
       (SELECT IFNULL(SUM(u.input_tokens+u.output_tokens),0) FROM usage u WHERE u.run_id=a.run_id AND u.agent_id=a.agent_id) AS tokens,
       (SELECT IFNULL(SUM(u.cost_usd),0) FROM usage u WHERE u.run_id=a.run_id AND u.agent_id=a.agent_id) AS cost_usd
FROM agents a
WHERE a.run_id = ?1 AND a.phase_seq = ?2
ORDER BY a.start_seq;

-- Q4 Tool-call timeline for one agent (step detail / IoDisclosurePanel).
SELECT seq, tool_name, args_json, result_digest, ts
FROM tool_calls
WHERE run_id = ?1 AND agent_id = ?2
ORDER BY seq;

-- Q5 Token/cost rollup grouped by phase (cost attribution; feeds dim per-phase metric).
SELECT p.start_seq, p.phase,
       IFNULL(SUM(u.input_tokens),0)  AS input_tokens,
       IFNULL(SUM(u.output_tokens),0) AS output_tokens,
       IFNULL(SUM(u.cost_usd),0)      AS cost_usd
FROM phases p
LEFT JOIN usage u ON u.run_id = p.run_id AND u.phase_seq = p.start_seq
WHERE p.run_id = ?1
GROUP BY p.start_seq, p.phase
ORDER BY p.start_seq;

-- Q6 Live tail (poll loop): everything newer than the last seq the UI rendered.
SELECT seq, event, ts, phase, agent_id, payload
FROM events
WHERE run_id = ?1 AND seq > ?2
ORDER BY seq;

-- Q7 Runs list (home screen): newest first, live runs surfaced.
SELECT run_id, workflow, started_ts, COALESCE(status,'running') AS status,
       (SELECT IFNULL(SUM(input_tokens+output_tokens),0) FROM usage u WHERE u.run_id=r.run_id) AS tokens
FROM runs r ORDER BY started_ts DESC LIMIT 50;

-- Q8 Resume preview: which checkpoints already have a recorded value
-- (a resume would short-circuit these).
SELECT key, content_key, value_digest, ts FROM checkpoints WHERE run_id = ?1 ORDER BY seq;
```

### 4.3 Interactions (from the UX spec)

- **Expand a step to see I/O** — *the primary interaction.* Click an AgentRow /
  ToolCallTwig / CheckpointLine to slide open an inline monospace panel with its
  input arg and output string/digest (and tool args). Collapsed by default. Inline,
  **no modal or drawer** — exactly like inspecting a step in Claude Code.
- **Collapse/expand a phase group** — click a PhaseGroupRow to fold/unfold its
  child agent + log rows; a global *collapse-to-phases / expand-all* toggle does it
  at once. Default final state on `run.ended` is collapsed-to-phases.
- **Follow-live (tail)** — toggle ON by default while running: auto-scrolls and
  streams newly-appended rows with the active-leaf spinner pinned in view. Toggling
  OFF freezes scroll. Auto-disengages on `run.ended`.
- **Filter** — a single text box that narrows the tree to rows matching a phase
  label, agent name, or tool name (keeping ancestor phase headers for context) — a
  *find-in-tree*, not a faceted query builder.

---

## 5. How it stays live

A **tail-and-ingest loop — no daemon, no heavy server.** This is the structured
upgrade of `tail -f | jq`.

**Mechanics.** The run directory writes `events.jsonl` flush-per-event. The
ingester keeps a `byte_offset` per run in `ingest_cursor`. On each tick it:

1. opens `events.jsonl`, seeks to `byte_offset`, reads only the new tail;
2. splits on `\n` and **drops a trailing partial line** (a half-written record —
   don't parse until the `\n` arrives);
3. `json/decode`s each complete line;
4. runs the idempotent projection inside **one transaction per batch**
   (`db/exec-batch "BEGIN; … ; COMMIT"`);
5. advances `byte_offset` to the last complete-line boundary and bumps `last_seq`.

Because projection is `INSERT OR IGNORE` on `(run_id, seq)`, a crash mid-batch just
re-reads the same tail next tick and converges — **at-least-once delivery with
exactly-once effect.**

**Two trigger modes:** (a) cheap fixed-interval poll (~250 ms–1 s) driven by the
viewer; the monotonic `seq` means the UI only asks `seq > N` (Q6) and never
re-renders settled rows; (b) optional filesystem watch to ingest on change instead
of polling.

**Concurrency.** Open the DB once with `PRAGMA journal_mode=WAL` (already done by
`db/open`) + `PRAGMA synchronous=NORMAL` so the viewer's reads never block the
ingester's appends and vice-versa — **single writer (the ingester), many readers
(each query).** SQLite WAL's sweet spot.

A run is **live iff `runs.ended_ts IS NULL`.** The UI polls those `run_id`s and
stops when `run.ended` lands. Because the DB is a disposable projection, **"live"
and "rebuild" are the same code path:** a fresh viewer with an empty DB replays
from offset 0, catches up, then tails — no cold-start vs warm-start distinction,
no separate backfill job, no schema lock-in.

---

## 6. Build phasing

**Hard precondition:** this milestone starts **only after Spike 1 freezes the
~8-event journal vocabulary** and ships a golden `events.jsonl` (derisk Spike 1;
scoping open-question #5). The schema, ingester, and viewer all read the frozen
shape — building against an unfrozen vocabulary guarantees rework. This is its
**own milestone**, sequenced after the journal, parallel-izable with later
workflow spikes (resume, parallel fan-out) since it only *reads* the journal.

### 6.1 MVP

1. **Schema + ingester (Sema).** Create the §3.1 tables; implement the §3.2
   replay-to-rows dispatcher over the shipped sqlite stdlib. **Oracle:** replay
   Spike 1's golden `events.jsonl` → assert row counts and a fixed projection
   snapshot; replay twice → byte-identical DB content (idempotency). Run under the
   fixed-ts seam (`dur_ms = 0`) so the projection is deterministic.
2. **`sema workflow view` server.** axum router cloned from the notebook server
   (loopback default, `include_str!` UI, no auth), running the §5 ingester loop and
   exposing `GET /api/runs`, `/api/run/:id`, `/api/run/:id/tree`,
   `/api/run/:id/events?since=:seq`, `/api/agent/:run/:agent`.
3. **The tree viewer (HTML).** Promote the prototype to the embedded UI: run
   header + meta strip, phase group rows, agent rows mutating in place, tool twigs,
   narrator/checkpoint/budget lines, expand-to-see-I/O, status glyphs, token meter.
   **Follow-live polling** (Q6 cursor) with the active-leaf spinner.
4. **Finished-run rendering.** `run.ended` → freeze to a static summary collapsed
   to phase level, re-expandable; `FinalEnvelopeBar`.

This MVP delivers the full single-run live tree — the entire §1 aesthetic — over a
finished or a live Spike-1 sequential run.

### 6.2 Later

- **Concurrency visualization at scale.** Once parallel fan-out lands (later
  workflow spike), verify N-simultaneous-spinner rendering against a real bounded
  `:max-concurrent` run. (The schema already supports it — concurrency is *derived*
  from overlapping `started → result` windows under one phase; nothing new to
  store.)
- **Filesystem-watch trigger** as an alternative to polling.
- **Runs list / home screen** (Q7) for browsing multiple runs — still single-run
  *views*, no cross-run analytics.
- **Deeper I/O disclosure** that drills from a `value_digest` into the real
  `checkpoints/` file on disk (the projection stays thin; the journal/disk owns the
  lossy bytes).
- **Static-file export** (Option A): bake a finished run into a self-contained
  `.html` for sharing with nothing installed.
- **OTel/notebook cross-render** parity, *if* it ever earns its keep — explicitly
  not in this milestone.

### 6.3 Out of scope for this milestone (forever, for the single-run view)

Everything in §1.2 — charts, gauges, KPI tiles, Gantt/swimlanes, DAG canvases,
multi-run analytics, decorative card chrome. If a reviewer sees one appear, it is a
regression against this scope.

---

## Appendix — why SQLite is a projection, not the system of record

Three guarantees earn the "projection" label:

1. **Derivability.** Every typed row is reconstructable by replaying the journal —
   `events` is a verbatim 1:1 landing of each line, and the typed tables are pure
   functions of it. The DB is disposable: delete it and replay from byte 0 yields
   byte-identical rows, so a schema change is *drop + re-replay*, never a migration
   of precious data.
2. **No authority leaks back to SQLite.** Nothing writes to it except the ingester;
   the workflow runtime never reads it to make decisions (resume reads
   `events.jsonl` + `checkpoints/` on disk), so the projection can lag, be rebuilt,
   or be absent without affecting correctness.
3. **It stays thin by deferring to the journal for anything lossy.** Agent output
   and tool results are stored as opaque string/digest (matching the frozen
   "`agent.result.output` is an opaque string" decision); the `payload` column is
   the lossless escape hatch — the viewer drills into the real value via
   `checkpoints/` files, not a fat blob in SQLite.

This rides the **shipped** sqlite layer (rusqlite 0.40 bundled;
`db/open|exec|exec-batch|query` in `crates/sema-stdlib/src/sqlite.rs:39+`) so the
whole projector is writable in Sema with **zero new Rust**. The `(run_id, seq)`
primary keys turn idempotency into a structural property (`INSERT OR IGNORE`),
exactly what a re-runnable projection of a monotonic-seq journal wants — matching
the runtime's own checkpoint-as-idempotent-memoization model. Per-event `usage`
rows (never a mutated counter) preserve the LLM accounting invariant: a cache hit
reports zero usage, so `SUM` never double-charges.
