# sema-web adversarial pass - 2026-07-28

37 findings, **all confirmed by execution**: each agent ran its own repro, and
unverified hypotheses were excluded by construction. Produced by a six-angle
parallel break phase against the nine sema-web gaps closed in this wave.

Severity as found: 3 critical, 11 high, 13 medium, 10 low.

## Status after the harden phase (reconciled 2026-07-28)

**34 FIXED, 1 PARTIALLY FIXED, 2 CLOSED BY DECISION.** Every finding below carries
its own `**Status …**` line naming the regression test that covers it; this table
is only the roll-up.

| Verdict | Findings |
| --- | --- |
| FIXED | 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 30, 31, 32, 33, 35, 36, 37 |
| PARTIALLY FIXED | 29 (the diagnosis names the cause; the ~100 wasted renders remain) |
| CLOSED BY DECISION | 11 (every re-rendering shape refetches; for a spec reading a signal the view never reads, the documented answer is `resource/refresh!` from an `effect` — docs shipped), 34 (`.once` migration in an unkeyed list — fixing it would re-arm a spent `.once` whenever a label changes, which is worse; the `sip-render:once-without-key` dev diagnostic and docs shipped instead) |

Three of those are answered by *documentation* rather than by a behaviour change,
each because this ledger's own **Expected** offered that as an alternative and the
code change was judged worse than the defect: **20** (effect re-run ownership),
**11**'s residual (a spec whose signal the view never reads), and **34**. They are
called out individually below rather than being quietly folded into "FIXED", and
**20**'s two tests are flagged in place as contract coverage rather than as a
regression witness — they pass before and after by construction.

(**11**'s residual and **34** were reclassified CLOSED BY DECISION on
2026-07-29: everything they resolve to has shipped — the docs in each case, and
for **34** the `once-without-key` dev diagnostic, live at
`packages/sema-web/src/sip.ts:389`.)

Verification behind this reconciliation, exit codes checked (2026-07-28):
`npm test` 996 passed / 39 files; `tsc --noEmit` clean; `npm run build` clean;
Playwright 108 passed; `cargo nextest run --workspace` 7306 tests with 7305
passing. Baseline at the start of the wave was 846 unit tests / 30 files and 79
Playwright tests.

Two flakes surfaced during that verification, both **unrelated to sema-web** and
both recorded here so the next reader does not re-derive them:

- `sema-lang::llm_root_nonblocking_test::root_rate_limit_pacing_parks_while_a_sibling_runs`
  — wall-clock sensitive (a `(sleep 10)` racing a 10 req/s pace), fails only
  under full-workspace parallel load; passes 3/3 in isolation.
- `sema-lsp::builtin_docs::tests::aliases_share_same_rc_entry` — order-dependent
  (1 failure in 15 isolated runs) *and* backed by a real data defect: the docs
  index carries `number->string` **twice**, as its own canonical entry
  (`crates/sema-docs/entries/stdlib/math/number-to-string.md`, module `math`,
  documents the `radix` argument) and as the declared alias of `number/to-string`
  (`crates/sema-docs/entries/stdlib/strings/number-to-string.md`, module
  `strings`, one param). `BuiltinDocs::load` therefore binds that name to two
  different `Rc<DocEntry>`s and the test's `Rc::ptr_eq` fails whenever hash-map
  iteration reaches `number/to-string` first. The stdlib really does register
  both names (`crates/sema-stdlib/src/string.rs:515,1633`), so the fix is to
  merge the two entries — an editorial call for a Rust/docs owner, not a
  sema-web change. **Closed** by commit `146f7654` (2026-07-28): the alias
  declaration was removed from
  `crates/sema-docs/entries/stdlib/strings/number-to-string.md`, leaving the
  `math` entry as the one canonical `number->string`; the previously flaky
  test passed 20/20 after.

One environmental flake, also not a product defect: with the default worker count
on a loaded machine the Playwright suite can lose a single `composition.spec.ts`
test to a 30s `beforeEach` browser-launch timeout (`browser.newContext: Test
ended`, no assertion failure). That spec passes 3/3 in isolation and the full
suite is 108/108 at `--workers=2`. `playwright.config.ts` sets `retries: 2` under
CI, so CI self-heals.

## The root cause behind most of the criticals

Five of the worst findings are one architectural gap seen from five angles:
**component-scoped state is keyed to the MOUNTED component, not to the composed
child instance.**

`component/render` invokes a child as an ordinary Sema function during the parent's
render, so `ctx.renderContextStack` still names the parent and the child's
`effect` / `on-unmount` / `local` / `resource` registrations land in the *parent's*
slot list. Slots are then matched positionally. Consequences:

- switching a branch keeps the old child's effect running and never runs the new one's
- removing a keyed row tears down a different row's effect
- every row of a list shares one `local` cell and one named resource

Fixing that single thing should close findings 1, 2, 3, 7 and 8 together. It needs a
per-child-instance identity in `component/render` - a change to the component model,
not a patch.

**Done 2026-07-28.** `component/render` brackets each child with
`__component/enter-child` / `__component/exit-child`; `currentScopePath(ctx)` in
`packages/sema-web/src/context.ts` yields the path, and `effect` / `on-unmount` /
`local` / `resource` all key off it. `disposeScope` releases a departed child's
slots, local cells and resources. Repeated children need `:key` in their **props**
(the SIP `:key` attribute is invisible to the host) - documented in
`website/docs/web/components.md`. Regression coverage:
`packages/sema-web/tests/component-child-scope.test.ts`, which drives the real
interpreter against real WASM because the defect lives in the interaction between
the `component/render` macro and the host's scope bookkeeping.

(The original text of this paragraph said "findings 1, 2, 3, 5 and 6", carried
over from the break phase's per-angle numbering. In the final numbering the five
are 1, 2, 3, 7 and 8.)

## Coverage by angle

| Angle | Findings |
| --- | --- |
| packages/sema-web gap 5 (src/router.ts — :query, not-found, router/link) and gap 10 (src/testing.ts  | 4 |
| @sema-lang/sema-web (/Users/helge/code/sema/sema/packages/sema-web) — adversarial cross-feature pass | 7 |
| Gap 3 (component lifecycle: effect / on-unmount) in /Users/helge/code/sema/sema/packages/sema-web —  | 7 |
| Gap 4 — /Users/helge/code/sema/sema/packages/sema-web/src/resource.ts (async resources: `resource`,  | 6 |
| Gap 6 (event modifiers in src/sip.ts, form helpers in src/dom.ts, EventDelegator in src/component.ts | 7 |
| packages/sema-web (@sema-lang/sema-web) plus a workspace-wide sweep of crates/ for the debug_assert  | 6 |

## Findings

### 1. [CRITICAL] Effect/on-unmount slots inside a keyed-list child are positional, so removing a row tears down the WRONG row's effect and orphans the removed row's

**Status 2026-07-28 (harden phase): FIXED.** `component/render` now brackets each child with `__component/enter-child` / `__component/exit-child`, so a child's lifecycle slots live under its own scope path (keyed by the `:key` in its **props**) instead of the mounting component's flat array. Witness: `packages/sema-web/tests/component-child-scope.test.ts` -> "tears down only the removed row's effect, and leaves its siblings running" (real interpreter, real WASM).

**Repro.** Fixture: `(defcomponent kid (props) (effect (list) (fn () (let ((id (js/set-interval (string-append "tick-" (:name props)) 30))) (fn () (js/clear-interval id))))) [:li {:key (:id props)} ...])` rendered as `(map (fn (r) (component/render kid r)) @rows)` over rows a,b,c, with one tick counter per row. Then `(put! rows (cdr @rows))` — remove row a. Real Chromium. Root cause: `component/render` does not push a render scope, so every child's `(effect ...)` lands in the MOUNTED ROOT's single `ownedLifecycleSlots` array (src/component.ts:1045 `__component/effect` → `requireRenderingComponent` at :435, flushed positionally by `flushLifecycle` at src/component.ts:281-308). `:key` gives the DOM stable identity; the slot array has none.

**Observed.** Row a leaves the DOM but its interval keeps firing forever: ticks-a 19 → 38 over 500ms. Row c is still rendered but its interval was cleared: ticks-c frozen at 17. `ctx.intervals.size` went 3 → 2, so the runtime believes it cleaned up correctly. A second probe with a logging cleanup shows it plainly: removing row `a` logs `close:c` and never logs `close:a`. Since `(effect (list) ...)` has empty deps that always compare equal, slot i keeps row a's live registration while positionally sitting under row b, and the prune loop (component.ts:290) only ever retires from the TAIL.

**Expected.** Removing row a should run row a's cleanup and only row a's; rows b and c, still on screen, should keep their effects running. Effect identity should follow the same key that already governs DOM identity.

### 2. [CRITICAL] A child rendered with component/render registers its effects and on-unmount into the PARENT's slot list, so branch switching keeps the old child's effect alive and never runs the new child's

**Status 2026-07-28 (harden phase): FIXED.** Same render-scope change: a branch switch is a scope departure, so the old child's cleanup runs and the new child's body runs. Witness: `tests/component-child-scope.test.ts` -> "runs the new child's effect and the old child's cleanup when a branch switches"; browser side in `e2e/tests/composition.spec.ts`.

**Repro.** Real Chrome + real WASM. `(defcomponent kid-a (props) (effect (list) (fn () (note "A-run") (fn () (note "A-clean")))) (on-unmount (fn () (note "A-down"))) [:div "A"])`, an identical `kid-b` with B-labelled hooks, and `(defcomponent shell () [:main (if (equal? @route "a") (component/render kid-a {}) (component/render kid-b {}))])`; `(mount! "#e" shell)`, then `(put! route "b")`, then `(component/unmount! "#e")`. This is the routing pattern the README (line ~365) and website/docs/web/routing.md show: a cond over component/render. Mechanism: component/render calls the child as a plain Sema function during the parent's render, so ctx.renderContextStack top is still the parent and requireRenderingComponent (src/component.ts:435) hands the child the PARENT's MountedComponent; flushLifecycle (src/component.ts:295-306) then matches slots by index+kind+deps only.

**Observed.** Log after mount = "A-run;". After `(put! route "b")` the DOM correctly shows B but the log is UNCHANGED — kid-b's effect body never ran and kid-a's cleanup never ran, because kid-a's slot 0 (kind effect, deps []) and kid-b's slot 0 (kind effect, deps []) compare equal and the framework keeps the running registration. After unmount the log is "A-run;A-down;A-clean;": the departed child's hooks fire at parent teardown and kid-b's on-unmount never fires at all. Any resource kid-a's effect owns (interval, subscription, stream) outlives kid-a for the whole life of the parent. No error is reported anywhere.

**Expected.** Either a child's lifecycle registrations are owned/keyed per child instance, or `(effect …)`/`(on-unmount …)` inside a component invoked via component/render is rejected with an error. Silently attributing one child's effect to a different child is the worst of the three options. The docs make it worse: README:345-349 and website/docs/web/components.md:138-142 say only "put effect and on-unmount at the top level of the component body" — which both children do — and never mention that a component/render child shares the parent's slots.

### 3. [CRITICAL] Resource names are keyed to the mounted component, not the composed child, so every `component/render` of the same child shares one resource and one request

**Status 2026-07-28 (harden phase): FIXED.** The resource memo key is now `<ownerId|global>:<scope path>:<name>`, so two `component/render` calls of the same child are two resources. Witness: `tests/component-child-scope.test.ts` -> "gives each composed child its own resource, not one shared by all" (real WASM; asserts both specs ran and each card rendered its own URL).

**Repro.** Real browser (Chrome, real WASM, real network), throwaway fixture: `(defcomponent user-card (props) (def u (resource "user" (fn () (string-append "/api/user/" (:id props))))) [:li {:data-testid (string-append "card-" (:id props))} (if (:loading @u) "loading" (:name (:value @u)))])` and `(defcomponent page () [:ul (component/render user-card {:id "1"}) (component/render user-card {:id "2"})])`, mounted with `(mount! "#app" page)`. page.route recorded every /api/user/* request. Reproduced identically in jsdom+real-WASM via renderSema, and in the mock-interpreter suite. Same failure for a keyed list: `[:ul (map (fn (n) (component/render row {:id n})) (list "1" "2" "3"))]` with `(resource "row" ...)` inside `row`.

**Observed.** Only ONE request ever leaves the page: `/api/user/2`. DOM: `<li data-testid="card-1">Bob</li><li data-testid="card-2">Bob</li>` — card 1 renders user 2's data. ctx.resources.size === 1, the only key is `1:user` (the PARENT's instanceId), and errors is []. The keyed-list variant is worse: one fetch to `/api/row/3` and all three `<li>` render row 3's payload. Root cause: resource.ts:245-246 builds the memo key as `${getCurrentOwnerId(ctx) ?? "global"}:${name}`, but `component/render` is a plain Sema call inside the parent's render (COMPONENT_SEMA_PRELUDE -> __component-render-guarded), so getCurrentOwnerId returns the parent's instanceId for every child instance. The second child's `existing.replaceSpec(specValue)` then silently makes the last child's spec win and releases the first child's callback handle. The same collision hits two top-level `(resource "cfg" ...)` calls in different modules, which both land on `global:cfg`.

**Expected.** Each composed child instance should get its own resource, so both URLs are requested and each card renders its own data. At minimum the collision should be loud (a duplicate-name diagnostic) rather than silently serving one child's data to all of them. The module docstring's claim that a resource is "memoized per component instance" is only true for *mounted* components, not for the composition model gap 1 shipped in the same wave.

### 4. [HIGH] Literal route segments are never percent-decoded, so any route pattern with a non-ASCII or reserved character silently never matches

**Status 2026-07-28 (harden phase): FIXED.** `literalRoutePattern()` compiles every literal character of a route pattern into an alternation of itself and its percent-encoded UTF-8 bytes (hex-case-insensitive), so a raw pattern matches the encoded URL a browser actually stores. `/` is exempt, so `%2F` still cannot satisfy a segment separator. Witness: `tests/router.test.ts` -> the "percent-encoded literal segments" block (8 tests, incl. astral-plane and lower-case escapes) plus `e2e/tests/router.spec.ts` in real Chrome.

**Repro.** Real interpreter, jsdom, and confirmed in real Chrome.

  const screen = await renderSema(`
    (router/init! {:mode :hash
                   :not-found "not-found-page"
                   :routes {"/" "home" "/søk" "search-page" "/tags/:t" "tag-page"}})
    (defcomponent view () [:div (str (:handler (router/current-route)))])`,
    { mount: "view" });
  screen.run('(router/push! "/søk")');

Root cause: router.ts:459-490 `navigate` writes `window.location.hash = "#/søk"`; every browser percent-encodes the fragment (real Chrome: `location.hash = "#/søk"` reads back `"#/s%C3%B8k"`; also `"#/a b"`->`"#/a%20b"`, `"#/日本"`->`"#/%E6%97%A5%E6%9C%AC"`, `'#/x"y'`->`"#/x%22y"`). router.ts:366-378 `matchRoute` then tests the ENCODED location against the raw pattern regex. Only PARAMS are decoded (`decodeUriComponentSafe(match[i+1])` at line 372); literal segments never are. Identical failure in `:mode :history` (`/s%C3%B8k` vs pattern `/søk`).

**Observed.** `(:handler (router/current-route))` is `"not-found-page"`; the rendered DOM is `<div>not-found-page</div>`; the route is `{:handler "not-found-page" :params {} :path "/s%C3%B8k" :query {}}` and the diagnostic reads `"/s%C3%B8k -> (no match) -> not-found-page"`. With no `:not-found` configured the route is plain `nil` and the view renders nothing. The router's own link is equally broken: `(router/link "/søk" "Søk" nil)` renders `<a data-sema-router-link="/søk" href="#/søk">` and clicking it leaves `route = nil` while the hash becomes `#/s%C3%B8k`. `/tags/søk` against `/tags/:t` DOES work (param path decodes to "søk"), which makes the literal-segment failure look arbitrary.

**Expected.** `"search-page"`. A route pattern the developer registered should be reachable by `router/push!`, by `router/link`, and by a user typing the URL. `matchRoute` should compare decoded-against-decoded (or compile patterns in encoded form) the same way it already decodes params.

### 5. [HIGH] EXTERNAL_PATH_RE misses the backslash form, so `/\host` is accepted as an internal path and renders a cross-origin link (history mode) plus an uncaught SecurityError

**Status 2026-07-28 (harden phase): FIXED.** Two changes: `EXTERNAL_PATH_RE` rejects any leading slash-ish pair from `{/,\}` plus a lone leading backslash, and `navigate()`'s `pushState`/`replaceState`/hash writes are wrapped and routed to `ctx.onerror` under `router:navigate`. Witness: `tests/router.test.ts` -> "refuses the backslash spellings of a protocol-relative path", "renders no anchor a browser would resolve off-origin from a backslash", "refuses a backslash path instead of leaving the origin"; plus `e2e/tests/router.spec.ts`.

**Repro.** Real interpreter:

  const screen = await renderSema(`
    (router/init! {:mode :history :routes {"/" "home" "/a" "va"}})
    (defcomponent view () [:div (router/link "/\\evil.example" "go" nil)])`,
    { mount: "view" });
  screen.click("a");

Also `router/href` and `router/push!`/`router/replace!` with the same argument. Root cause: router.ts:93 `EXTERNAL_PATH_RE = /^(?:[a-zA-Z][a-zA-Z0-9+.-]*:|\/\/)/` blocks `//host` but not `/\host`, `\\host`, or `/\/host` — all of which the WHATWG URL parser resolves identically to `//host` for special schemes. Verified in real Chrome: `document.getElementById("k").href` for `<a href="/\evil.example">` is `http://evil.example/`, byte-identical to the `//evil.example` control, and a plain click actually left the origin (page.url() became `chrome-error://chromewebdata/`, `net::ERR_NAME_NOT_RESOLVED` for evil.example).

**Observed.** No error is reported anywhere. `router/href` returns `"/\evil.example"`, and `router/link` renders `<a data-sema-router-link="/\evil.example" href="/\evil.example">go</a>` — the browser resolves that href to `http://evil.example/`, so the status bar, ctrl/cmd-click, middle-click, and right-click->"Open in new tab" all go off-site (handleLinkClick at router.ts:505-506 deliberately leaves modified and non-primary clicks to the browser). A plain click is intercepted, and `navigate` (router.ts:473-477) then calls `history.pushState` unguarded, which throws `SecurityError: pushState() cannot update history to the URL http://evil.example/` — reported by vitest as an UNHANDLED error, not routed through `ctx.onerror`. Real Chrome throws the same SecurityError. Control: `router/href("//evil.example")` correctly returns null and reports one error.

**Expected.** `/\evil.example` should be refused exactly like `//evil.example`: `router/href` -> null, `router/link` -> a `<span>` with a reported error, `router/push!` -> reported and ignored. This is precisely what the doc comment at router.ts:85-92 claims the regex prevents ("in history mode `<a href="//evil.example">` is a cross-origin navigation, and the router would have rendered it as if it were an internal route"). Separately, `navigate`'s `pushState`/`replaceState` calls should be try/caught and routed to `ctx.onerror` like `applyNavigationEffects` already is, so no history-API rejection can escape a delegated click handler. Note hash mode (the default) is unaffected — the href becomes the harmless fragment `#/\evil.example`.

### 6. [HIGH] A resource whose spec depends on props/state never refetches — the view renders stale data with :loading false and :error nil

**Status 2026-07-28 (harden phase): FIXED.** `replaceSpec` now schedules a coalesced microtask that re-resolves the adopted spec once (never during a render) and compares a fingerprint of the resolved *request* (url, method, sorted headers, body, credentials, `:as`) with the one the current data came from: moved -> `markLoading()` + a fresh attempt, identical -> stop. Witness: `tests/resource.test.ts` -> "refetches when a re-render resolves the spec to a different URL", "supersedes an in-flight attempt when the spec moves"; the loop guard is pinned by "does not refetch when re-renders resolve the same request"; browser coverage in `e2e/tests/resource.spec.ts` (`page.route` records which URLs actually left the page).

**Repro.** Minimal, no router: `(defcomponent view () (let ((res (resource "detail" (fn () (string-append "/d/" @who ".json"))))) [:div [:p @who] [:p (:got (:value (deref res)))] ...]))`, then click a button that does `(put! who "b")`. Also reproduced through the router: routes `/slow/:id`, resource URL built from `(:id (:params r))`, click `/slow/1` then `/slow/2` while the first fetch is still in flight. Root cause: `createResource` short-circuits to `replaceSpec` for an existing named resource (src/resource.ts:248-254), and `replaceSpec` (src/resource.ts:387-398) swaps the callback but never calls `scheduleAttempt()`.

**Observed.** `who` renders "b" while the resource renders "a"; network log shows exactly one fetch (`/d/a.json`). Router variant: `slow-id` renders "2" while the resource renders payload `{who:"1"}`, and `/slow/2.json` is NEVER requested. `:loading` is false and `:error` is nil, so nothing in the state signals staleness and nothing reaches `onerror`. An explicit `(resource/refresh! "detail")` does fetch the new URL, proving the swapped spec is live but unscheduled.

**Expected.** Gap 4's own candidate API in docs/plans/archive/2026-07-02-sema-web-framework-gaps.md:198-206 is `(resource (fn () (http/get (string-append "/api/users/" (:id props)))))` under the stated goal "load data, expose loading/error/value, rerender when it changes". A spec whose value changed should schedule a fresh attempt (or the state should at least expose that it is stale). NOTE: tests/resource.test.ts:726 deliberately encodes today's behaviour by calling `refresh(id)` manually, so this may be an intentional design choice — but it is documented nowhere user-facing (`resource` appears in no README section and no website/docs/web/*.md page), and it silently breaks the single most natural usage.

### 7. [HIGH] A named resource inside a list or route child collapses to ONE shared resource keyed by the mounted root, and is never disposed when the child goes away

**Status 2026-07-28 (harden phase): FIXED.** Same scope key as finding 3, plus `disposeScope` draining the departed child's `ownedScopeResources`. Witness: `tests/component-child-scope.test.ts` -> "disposes a departed child's resource" (`snapshot().resources` 1 -> 0) and the unmount test in `e2e/tests/resource.spec.ts` (resources / resourcesByKey / streams all drain to 0).

**Repro.** Three keyed rows, each `(defcomponent kid (props) ... (resource "row" (fn () (string-append "/data/" (:name props) ".json"))))` rendered via `(map (fn (r) (component/render kid r)) @rows)`; each row renders its own resource handle and value. Separately: a route view `slow` that owns `(resource "detail" ...)`, navigated away from back to `/`. Root cause: the memo key is `` `${ownerId ?? "global"}:${name}` `` (src/resource.ts:246) and `getCurrentOwnerId` resolves to the mounted root because `component/render` pushes no owner — children are not mounted components.

**Observed.** All three rows report resource handle **5**; `ctx.resourcesByKey` holds the single key `"1:row"`; `ctx.resources.size` is 1. Exactly ONE fetch is issued — `c.json`, the LAST child's spec, because each sibling's `replaceSpec` overwrote the previous one during the same render — and all three rows display row c's payload (`who: "c"`). No error anywhere. Navigation variant: after leaving `/slow/2` back to `/`, the route view is gone from the DOM but `ctx.resources.size` is still 1, `ctx.streams` still `[3]`, and `resourcesByKey` still `["1:detail"]`; it survives until `web.dispose()`.

**Expected.** Either a per-child resource identity (so N rows get N resources), or a hard error when two live callers claim the same resource name — silently serving one row's data to every row is the worst available outcome. And a resource created inside a child should be reaped when that child stops rendering, matching the plan's acceptance criterion "Unmount aborts the resource and removes owned stream/request state".

### 8. [HIGH] `local` inside a list child is one shared state cell across all rows, and per-row cells are never released when a row is removed

**Status 2026-07-28 (harden phase): FIXED.** Both halves. Sharing: `local` keys off the render scope path, so each row gets its own cell. Release: `localScopes` on the mounted component plus `disposeLocalScope` in `disposeScope`. Witness: `tests/component-child-scope.test.ts` -> "gives each row of a list its own local cell", "keeps a child's state with it across a reorder", "releases a departed child's local cells", "releases the local cells of a child that owns nothing else". "keeps the mounting component's own local cells across renders" is the control.

**Repro.** (a) Sharing: `(defcomponent kid (props) ... (let ((seen (local "seen" 0))) ... [:span (number->string seen)]))` over three keyed rows via `component/render` inside `map`; render the raw signal handle. (b) Leak: give each row a distinct name — `(local (string-append "seen-" (:name props)) 0)` — then remove a row. Root cause: `__component/local` (src/component.ts:1018-1033) keys by name inside the mounted root's `localState` map; a child has no scope of its own, and nothing ever prunes an entry.

**Observed.** (a) All three rows return handle **4** — one signal shared by every row, so writing one row's "own" state writes all of them, with no error. (b) After removing row c, `component.localState` still contains `["seen-c", 7]` and signal 7 is still in `ctx.signals`. The map only ever grows for the life of the mounted root — for a churning list (search results, infinite scroll, a paginated table) this is unbounded.

**Expected.** `local` inside a child should be scoped to that child instance, and a cell belonging to a row that is no longer rendered should be disposed with the row. At minimum the shared-cell case should be detected in dev mode rather than silently aliasing.

### 9. [HIGH] Two live components sharing one named on-unmount function: the first unmount frees the shared callback handle, so the second component's teardown never runs

**Status 2026-07-28 (harden phase): FIXED.** `releaseCallback` in `src/callbacks.ts` is refcounted, so the first owner's teardown no longer frees a handle a second live owner still holds. Witness: `tests/component-callback-sharing.test.ts` (6 tests, incl. "runs both components' on-unmount hooks, in either teardown order" and "releases the shared handle once both owners are gone") and the real-WASM `tests/component-lifecycle-sema.test.ts` case, which reproduces the ledger's exact `"down"` vs `"down;down"` symptom.

**Repro.** Real Chrome + real WASM. `(define (shared-teardown) (note "down"))`, `(defcomponent leaf () (on-unmount shared-teardown) [:div "leaf"])`, `(mount! "#a" leaf)`, `(mount! "#b" leaf)`, then `(component/unmount! "#a")` followed by `(component/unmount! "#b")`. Mechanism: crates/sema-wasm/src/lib.rs allocate_callback_handle returns ONE handle per identical Sema value and release_callback removes it with no refcount, while packages/sema/src/index.ts memoizes one JS wrapper per handle — so both components' unmount slots hold the same wrapper. retireLifecycleSlot (src/component.ts:268) calls releaseCallback on it during the first teardown.

**Observed.** After unmounting #a the log is "down;". After unmounting #b the log is STILL "down;" and onerror receives `on-unmount:leaf#0: Unknown callback handle: 1`. The second component's teardown silently never executes — whatever it was meant to release stays leaked. Same root cause applies to any callback that is stored and invoked later: a named function returned as an effect cleanup, or an on-mount cleanup, shared by two instances or by two slots that retire at different times.

**Expected.** Both teardowns run ("down;down;") and no error is reported. Either the handle release must be refcounted, or a slot must not release a handle it cannot prove it uniquely owns. Note this makes `(on-unmount my-teardown)` — the natural way to reuse a teardown across two mount points or two rows of the same component — a silent-failure form, while `(on-unmount (fn () …))` and the documented "a global's name" string form are both safe.

### 10. [HIGH] component/force-render! called from inside an effect body recurses until the JS stack overflows, and permanently orphans a render effect that keeps re-rendering after unmount

**Status 2026-07-28 (harden phase): FIXED.** `renderComponent` carries a re-entrancy guard, so `component/force-render!` during a flush is refused with a diagnostic instead of recursing, and no orphan render effect survives unmount. Witness: `tests/component-reentrancy.test.ts` -> "is refused from an effect body instead of recursing", "leaves no orphaned render effect firing after unmount", "is refused from a render body"; "still re-renders when called from an event handler" is the control. Real Chrome: `e2e/tests/reentrancy.spec.ts`.

**Repro.** Real Chrome + real WASM. Register a JS render counter, then `(defcomponent boom () (probe/tick) (effect (list) (fn () (if (= @gen 0) (component/force-render! "#c") nil))) [:div (number->string @gen)])`, `(mount! "#c" boom)`; then `(put! gen 1)`, `(component/unmount! "#c")`, `(put! gen 2)`. Two mechanisms: (a) flushLifecycle assigns `live[index] = runLifecycleSlot(...)` (src/component.ts:305) only AFTER the body returns, so the nested render sees no live slot at that index and re-runs the same body — unbounded recursion; (b) renderComponent (src/component.ts:382-384) reads `component.dispose` before the outer `effect()` call has assigned it, so the nested effect's disposer is overwritten by the outer one and is never disposed.

**Observed.** 174 renders for a single mount, terminating with `effect:boom#0: Maximum call stack size exceeded`. Every subsequent state write costs another ~174 renders (348 after one `(put! gen 1)`). After `(component/unmount! "#c")`, `(put! gen 2)` still produces 173 more renders (521 total) of the destroyed component, each reporting `component:boom: … (effect) no active mounted component` — orphaned render effects, unreachable by unmount or dispose, firing forever. jsdom repro with a one-shot guard shows the isolated mechanism: 2 renders per write while mounted and 1 zombie render after unmount.

**Expected.** Either force-render! is a no-op / diagnosed error while a flush is in progress (the component is already re-rendering), or the nested render is reentrancy-guarded and its disposer tracked. Unbounded recursion plus an undisposable effect is the worst case: the error is caught so the page looks alive while every state write burns ~174 renders and post-unmount renders never stop.

### 11. [HIGH] A spec whose URL depends on props or signals never refetches when they change — the new spec is adopted but no attempt is scheduled

**Status 2026-07-28 (harden phase): FIXED for every re-rendering shape; residual CLOSED BY DECISION (2026-07-29).** Every shape where the component re-renders now refetches (props from a parent, a signal the view reads, a route param) via the finding-6 fingerprint check - including the plan's own `(fn () (http/get (string-append "/api/users/" (:id props))))`. Witness: `tests/resource.test.ts` -> "re-checks a spec whose closure identity never changes", "refetches when only the method, headers, or body move"; `tests/resource-sema.test.ts` and `e2e/tests/resource.spec.ts` drive it end to end.

**Residual, stated plainly - closed by decision.** This finding's *literal* repro renders `(:name (:value @u))` and never reads `@uid` in the view, so `(put! uid "2")` re-renders nothing and a re-render-driven check cannot see it. Making the spec's signal reads *tracked* was designed and rejected: a spec that writes a signal becomes self-triggering, and two specs that read each other's writes produce a microtask loop no fingerprint can break (a frozen tab). For that shape the documented answer is the ledger's own second Expected - `(effect (list @uid) (fn () (resource/refresh! "user")))` - now in the module docstring, `README.md` and `website/docs/web/resources.md`. With those docs shipped, nothing here remains open.

**Repro.** renderSema (real WASM): `(def uid (state "1"))` + `(defcomponent view () (def u (resource "user" (fn () (string-append "/api/user/" @uid)))) [:p (if (:loading @u) "..." (:name (:value @u)))])`, mounted; resolve the first response as {"name":"one"}; then `(put! uid "2")` and flush.

**Observed.** fetch URLs = ['/api/user/1'] only; the DOM still reads "one"; errors is []. The re-render reaches `existing.replaceSpec(specValue)` (resource.ts:252) which swaps the closure and releases the old handle, but never calls scheduleAttempt, so the resource keeps serving the response for uid=1 indefinitely. Nothing anywhere reports that the displayed data no longer matches the props that produced it.

**Expected.** The plan's own candidate API for this feature is `(resource (fn () (http/get (string-append "/api/users/" (:id props)))))`, i.e. a spec that closes over props — so a props change producing stale data with no signal is a silent-wrong-data trap. Either the resource needs a dependency key that re-triggers on change, or the documented pattern must be `(effect (list (:id props)) (fn () (resource/refresh! "user")))`. There is currently no user documentation for `resource` at all (no website/docs/web page, no README section), so a user has no way to learn the manual-refresh requirement.

### 12. [HIGH] A focused input keeps attributes a re-render removed — including a removed `data-sema-on-*`, whose handler keeps firing forever

**Status 2026-07-28 (harden phase): FIXED.** `onBeforeElUpdated` no longer hand-copies attributes: `syncAttributesPreservingValue` mirrors morphdom's `morphAttrs` (adds **and** removes, namespace-aware, `value` exempt both ways), so a removed `data-sema-on-*` really stops dispatching. Witness: `tests/focus-preservation.test.ts` -> "removes an attribute the new render no longer declares", "stops dispatching a handler the new render removed", "stops applying a modifier the new render dropped"; real Chrome in `e2e/tests/focus-preservation.spec.ts`.

**Repro.** Real Chrome, full Sema/WASM chain. Component: `(if (= @kcount 0) [:input {:id "i" :on-keydown "bump" :class "armed" :required true :aria-invalid "true"}] [:input {:id "i"}])`, where `bump` increments `kcount`. Focus #i, press a key (kcount 0→1, so render 2 declares a BARE input), then press a second key. jsdom equivalent reproduces identically, and the same scenario with the input NOT focused behaves correctly — that control is what isolates the cause. Cause: `sipMorphOptions.onBeforeElUpdated` in src/component.ts:353-365 copies every attribute from `toEl` onto `fromEl` and returns `false`; it never removes attributes present on `fromEl` but absent from `toEl`, and returning `false` also stops morphdom doing it.

**Observed.** Render 2 DOM is unchanged from render 1: `<input aria-invalid="true" class="armed" id="i" data-sema-on-keydown="bump" required="">`. The second keypress drives the counter to 2, i.e. the delegator dispatched a handler that the current render does not declare at all. `required`, `class="armed"` and `aria-invalid="true"` also persist. `data-sema-mods-keydown` persists the same way, so a removed `.prevent`/`.stop`/`.once` keeps applying. No error is reported anywhere.

**Expected.** Render 2 declares no handler and no class/required, so the second keypress should leave the counter at 1 and the element should be `<input id="i">`. website/docs/web/components.md states outright that with focus preservation "Attributes (like `class`) are still updated, but the `value` property is left alone" — attribute *removals* are silently exempt from that promise.

### 13. [HIGH] A typo in an `on-*` EVENT name is completely silent, so a `.prevent`ed form really does navigate away

**Status 2026-07-28 (harden phase): FIXED.** The delegator's event list is now `DELEGATED_EVENTS`, exported from `sip.ts` and consumed by both the delegator and a new `delegationError()`, which refuses any other `on-*` name through `ctx.onerror` under `sip-render:on-handler` with a Levenshtein "did you mean", a named stand-in for the non-bubbling pair (`focus` -> `focusin`), and `dom/on!` for custom-element events. The set grew from 14 to 43 events. Witness: `tests/sip-events.test.ts` -> "rejects a typo in the EVENT name, which the delegator could never route", "names the bubbling stand-in for an event that cannot be delegated", "rejects a custom event name and points at dom/on!"; `e2e/tests/event-routing.spec.ts` asserts in real Chrome that the typo'd form still navigates - the symptom the diagnostic exists for.

**Repro.** Real Chrome, full Sema/WASM chain. `[:form {:id "typo-form" :action "/navigated.html" :method "get" :on-sumbit.prevent "saved"} [:input {:type "hidden" :name "q" :value "1"}] [:button {:type "submit"} "Go"]]` — note `sumbit`, one transposition. Click the submit button. Cause: `parseEventAttrKey` (src/sip.ts:113-137) validates *modifiers* against a closed set and rejects an unknown one loudly, but validates the *event name* only against `EVENT_NAME_RE` — any identifier passes — while the delegator only ever listens for the 16 names hardcoded in `EventDelegator.setup` (src/component.ts:620-624).

**Observed.** SIP renders `data-sema-on-sumbit="saved" data-sema-mods-sumbit="prevent"`, `window.__semaErrors` is `[]`, and the page navigates to `http://localhost:5173/navigated.html?q=1`. The same fixture's correctly-spelled `:on-submit.prevent` form is prevented, so the delegator itself is fine. Separately, `on-focus`, `on-blur`, `on-mousedown`, `on-mouseup`, `on-mousemove`, `on-wheel`, `on-scroll`, `on-drop`, `on-dragover`, `on-paste`, `on-copy`, `on-reset`, `on-invalid` and `on-touchstart` all install a dead attribute with zero diagnostics.

**Expected.** An unroutable event name should be reported through `ctx.onerror` (or at minimum a dev-mode diagnostic) exactly as an unknown modifier is. `parseEventAttrKey`'s own TSDoc gives the reason: "a silently dropped `.prevnt` lets a form navigate away with no signal anywhere, which is the worst possible failure for a typo to have" — `:on-sumbit.prevent` produces that identical outcome and is not caught. website/docs/web/components.md also claims "All standard DOM events are supported via delegation", which is false.

### 14. [HIGH] CJS consumers cannot import the package at all, yet 186 KB of unreachable CJS output is built and shipped

**Status 2026-07-28 (harden phase): FIXED.** Resolved in the honest direction: `format: ["esm"]` only (no more unreachable CJS output), `main`/`types` are `./`-prefixed, and every `exports` subpath carries a `default` condition so a `require()` resolves rather than throwing `ERR_PACKAGE_PATH_NOT_EXPORTED`. Witness: `tests/package-boundary.test.ts` -> "resolves the runtime for a CJS requirer instead of refusing the subpath" and "ships no CJS output, because nothing could ever load it" - both run real `node` against a throwaway project whose `node_modules` symlinks this package, so they cross the package boundary the in-repo suite cannot. Verified in this pass: a fresh `npm run build` emits no `dist/index.cjs`.

**Repro.** cd /tmp && mkdir -p p/node_modules/@sema-lang && ln -s /Users/helge/code/sema/sema/packages/sema-web p/node_modules/@sema-lang/sema-web && printf '{"name":"p","version":"1.0.0"}' > p/package.json && cd p && node -e 'require("@sema-lang/sema-web")'   # then the type-check half: same dir, app.ts = `import { SemaWeb } from "@sema-lang/sema-web";`, tsconfig {module:node16, moduleResolution:node16}, run tsc

**Observed.** Runtime (Node v26.3.0): throws ERR_PACKAGE_PATH_NOT_EXPORTED — 'No "exports" main defined in .../packages/sema-web/package.json'. Type-check: error TS1479 ('The current file is a CommonJS module whose imports will produce require calls; however, the referenced file is an ECMAScript module and cannot be imported with require'); traceResolution shows TS resolving to dist/index.d.ts and never considering dist/index.cjs. Root cause: package.json:8 exports["."] is {"import":"./dist/index.js","types":"./dist/index.d.ts"} with no "require" and no "default" condition, so require() can match nothing; and package.json:6 main points at dist/index.js, the ESM file, so even a legacy resolver that ignores exports gets ESM. Meanwhile tsup.config.ts:9 declares format: ["esm","cjs"], so the build really does emit dist/index.cjs (134 KB) and dist/index.d.cts (52 KB), and files:["dist/"] publishes both — 186 KB of dead weight that no resolver can ever select. ESM import and the ./testing subpath both work fine, which is why this is invisible day-to-day.

**Expected.** Either exports["."] gains a "require": "./dist/index.cjs" (+ a types/require entry pointing at dist/index.d.cts) so the CJS build that is already being produced is actually reachable, or the cjs format is dropped from tsup.config.ts so the package is honestly ESM-only and stops shipping 186 KB of unreachable output. Note main should not point at the ESM file either.

### 15. [MEDIUM] renderSema resets document.body but never window.location, so router state leaks between screens and makes router tests order-dependent

**Status 2026-07-28 (harden phase): FIXED.** `renderSema` now calls `resetLocation()` alongside the body reset, and `RenderSemaOptions` gained a documented `url` field. Witness: `tests/testing.test.ts` -> the "starting URL" block, incl. "mounts on the route the url option names, whatever the ambient URL is" and "names the harness when the requested URL is not usable".

**Repro.** Two sequential renderSema calls in one file, second one identical to the first and doing no navigation of its own:

  window.location.hash = "#/a";
  const s1 = await renderSema(`(router/init! {"/a" "ha" "/b" "hb"})`, {});
  s1.run('(router/push! "/b")');
  s1.dispose();
  const s2 = await renderSema(`(router/init! {"/a" "ha" "/b" "hb"})`, {});
  s2.run("(str (router/current-route))");

testing.ts:421 does `document.body.innerHTML = options.html ?? DEFAULT_TEST_HTML` but nothing touches `window.location`; `RenderSemaOptions` has no `url`/`hash`/`path` field, so a router test has to poke `window.location` directly — which is exactly what then leaks into the next test.

**Observed.** s2 resolves to `{:handler "hb" :params {} :path "/b" :query {}}` and `window.location.hash` is still `#/b` — s2 boots on the route the PREVIOUS screen navigated to. `s1.dispose()` and `disposeAllScreens()` do not help; jsdom shares one `window` per file, so a router suite silently becomes order-dependent and reordering or `.only`-ing a test changes which route a component mounts on.

**Expected.** Either reset `window.location` to a known state (hash `""` / path `"/"`) alongside `document.body`, or add a documented `url`/`hash` option so a test can state its starting route without touching globals. `disposeAllScreens`' own doc promises "one forgotten dispose() cannot leak a WASM instance, a timer, or a document listener into the next test" — the URL is a fourth thing that leaks and nothing drains it.

### 16. [MEDIUM] `watch` and `computed` called from a component render body accumulate one live registration per render

**Status 2026-07-28 (harden phase): FIXED.** `watch` and `computed` called from a render body are memoized per render scope (scope path + signal id + occurrence), the way `local`/`resource` already were, and the post-render sweep disposes any key the render stopped claiming. A second defect found on the way: registering a watch took its baseline with a *tracked* read, silently subscribing the calling render to a signal it never reads. Witness: `tests/reactive-render-scope.test.ts` and `tests/reactive-render-scope-sema.test.ts` (20 tests; real-WASM "keeps one live watch across many renders" / "keeps one live computed across many renders"). The tracked-read half is pinned by "does not subscribe the render to the signal it watches".

**Repro.** `(defcomponent view () (watch other (fn (o v) nil)) (let ((doubled (computed (* @n 2)))) [:div [:p (number->string @n)] [:p (number->string @doubled)] [:button {:on-click "bump"} "+"]]))`, then click bump 30 times. Read `ctx.watchDisposers.size`, `ctx.signals.size`, `component.ownedSignalIds.size`, `ctx.signalFinalizers.size`. Root cause: `__state/watch` (src/reactive.ts:118-142) and `__state/computed-create` (src/reactive.ts:77-96) allocate unconditionally — unlike `local` and `resource`, which are memoized by name for exactly this reason (see the "Identity is by name" rationale at src/resource.ts:17-25), and unlike `effect`, which is slotted by index.

**Observed.** 31 renders → `watchDisposers.size` 31, `ownedWatchIds` 31, `ownedSignalIds` 31, `signalFinalizers` 31, `ctx.signals` 4 → 34. Every one of the 31 watches is live and invokes a Sema callback on each change to its signal (O(renders) work per write), and each holds a Sema callback handle in the WASM callback table. Nothing is released until unmount, and no diagnostic is recorded.

**Expected.** Either per-render memoization the way `local`/`resource` have it, or a dev-mode diagnostic when the same component registers a growing number of watches/computeds. Both primitives explicitly attribute ownership to the rendering component (`getActiveComponent()` → `owner.ownedWatchIds.add`), which is what makes in-render use look supported; the README's only nearby guidance (packages/sema-web/README.md:347) covers things created inside an *effect* body, not a render body.

### 17. [MEDIUM] Workaround-era comments claim the resolved section-38 WASM trap is still live, and the lifecycle e2e fixture avoids `update!` because of it

**Status 2026-07-28 (harden phase): FIXED.** All three files read as history. `e2e/fixtures/scripts/lifecycle.sema` describes section 38 in the past tense **and its interval callback now uses `update!`** - the exact form the workaround avoided - so the browser suite exercises the restored contract; `e2e/tests/lifecycle.spec.ts` and `e2e/fixtures/scripts/forms.sema` likewise. Re-swept in this pass across `src/`, `tests/`, `e2e/`, `README.md` and `website/docs/web/`: no present-tense claim remains. `docs/limitations.md` section 38's "consumers should be revisited" paragraph was also stale and now records that both consumers were fixed.

**Repro.** Read `e2e/fixtures/scripts/lifecycle.sema:5-8` and `:20-24`, `e2e/tests/lifecycle.spec.ts:8-10`, `e2e/fixtures/scripts/forms.sema:10-14`, against `docs/limitations.md:279` (`### ~~38. Host-invoked Sema calls trap the WASM VM~~ → RESOLVED (2026-07-28)`).

**Observed.** All three files assert the trap in the present tense: lifecycle.sema — "traps the VM with 'RuntimeError: unreachable' the way a host call inside a render's map callback does (docs/limitations.md 38)" and "`apply` inside any host-invoked Sema call — a render, an on-mount callback, a watch, an effect body — traps the WASM VM ... Effect bodies are host-invoked, so they live under the same rule"; lifecycle.spec.ts — "aborts the instance with 'RuntimeError: unreachable'"; forms.sema — "a host call inside a mounted render traps the WASM VM". Consequently `lifecycle.sema:25` uses `(define (bump-ticks) (put! ticks (+ @ticks 1)))` *specifically to avoid* `update!`, so the browser suite never exercises the contract the fix restored. I verified by hand in Chromium that `update!` (which goes through `apply`) works in an effect body, on-mount, a watch callback, and an event handler, and that a host call inside a mounted render's `map` callback is fine. `src/component.ts:857-864` is the one place that correctly describes it in the past tense.

**Expected.** The comments should read as history, not as a live constraint, and the lifecycle fixture should use `update!` in an effect body — that is now the intended contract and it is exactly the thing only a real browser can prove. As written, a reader will keep hand-writing the `put!`-with-explicit-read workaround indefinitely.

### 18. [MEDIUM] component/unmount! from inside an effect body silently discards that effect's cleanup and resurrects a slot on the destroyed component

**Status 2026-07-28 (harden phase): FIXED.** `flushScope` and `renderComponent` check for a destroyed component after every user-code call, so a body that unmounts its own component has its returned cleanup run and leaves no live slot behind. Witness: `tests/component-reentrancy.test.ts` -> the "component/unmount! from inside an effect body" block (6 tests), incl. "runs the cleanup the body returned instead of stranding it", "leaves no lifecycle slot behind on the destroyed component", "releases every callback handle the destroyed render was holding" and "runs a pruned slot's cleanup once when that cleanup unmounts the component" (a double-run found by the fix's own tests, not by this ledger).

**Repro.** Real Chrome + real WASM: `(defcomponent selfdestruct () (effect (list) (fn () (component/unmount! "#d") (fn () (note "d-cleanup")))) [:div "d"])`, `(mount! "#d" selfdestruct)`. jsdom variant confirms the bookkeeping. Mechanism: destroyMountedComponent drains ownedLifecycleSlots and pendingLifecycle (src/component.ts:508 -> disposeLifecycleSlots) while flushLifecycle is mid-loop, and flushLifecycle then executes `live[index] = runLifecycleSlot(...)` (src/component.ts:305) into the array it just emptied.

**Observed.** Real browser: the cleanup never runs (log stays empty) and nothing is reported — a returned cleanup silently vanishes. jsdom: after the call, `ownedLifecycleSlots` = 1 slot, `pendingLifecycle` = 0, the component is gone from ctx.mountedComponents/ById, and the cleanup was called 0 times. When the unmounting body is the SECOND effect, the array is left sparse — `[<1 empty item>, 'effect']` — so any later drain pops `undefined` and would throw inside retireLifecycleSlot; here it is simply unreachable garbage whose cleanups can never run.

**Expected.** A body that destroys its own component should either be rejected or have its registration discarded, with the returned cleanup run (or explicitly not created). Silently dropping a cleanup is a leak with no diagnostic, and leaving a sparse array of live slots on a destroyed component means the framework's own "leaves no lifecycle state behind after unmount" invariant (tests/component-lifecycle.test.ts:531) is violated by a legal program.

### 19. [MEDIUM] A render that changes its slot count makes an on-unmount hook run early AND again at teardown — teardown executes twice, with no guard and no dev diagnostic

**Status 2026-07-28 (harden phase): FIXED.** A slot is retired with a *reason*, so a hook that merely moved index is re-keyed rather than invoked as a cleanup; a slot-shape change is reported through the dev diagnostics channel. Witness: `tests/component-reentrancy.test.ts` -> "does not run an on-unmount hook that merely moved index", "reports the kind change that re-keyed the slots", "reports an on-unmount hook a later render stopped registering", "still prunes and cleans up an effect slot a later render drops".

**Repro.** Real Chrome + real WASM: `(defcomponent keyed () (effect (list) (fn () nil)) (if (> @phase 0) (effect (list) (fn () nil)) nil) (on-unmount (fn () (note "bye"))) [:div (number->string @phase)])`, `(mount! "#c" keyed)`, then `(put! phase 1)`, then unmount. jsdom shows the mirror case (dropping an effect above the hook) with the same result.

**Observed.** Log after `(put! phase 1)` is already "bye;" — the teardown hook fired while the component was still mounted and visible — and after unmount it is "bye;bye;". The hook ran twice. Cause: the on-unmount slot moves index, so flushLifecycle sees kind effect vs kind unmount at that index and retires the unmount slot (retireLifecycleSlot invokes it as a cleanup), then re-registers it fresh. Nothing is reported through onerror and nothing is recorded in Diagnostics.

**Expected.** The docs (website/docs/web/components.md:138-142, README:345-349) warn that a changed slot count "re-keys the ones that follow", but the actual consequence — your on-unmount hook fires mid-life and then fires a second time at teardown — is undocumented and undetectable. React hard-errors on a hook-count change; here the runtime has a Diagnostics channel and a render-count it could compare against, and warns about neither. At minimum a dev-mode diagnostic when a render's slot count or kind sequence changes.

### 20. [MEDIUM] A re-running effect accumulates one framework-owned watch/interval/stream per run, contradicting the documented promise that a cleanup is only needed for things the framework does not know about

**Status 2026-07-28 (harden phase): FIXED (as documentation - the behaviour is unchanged, deliberately).** The ledger offered both remedies ("scope effect-created resources to the slot ... or fix the sentence"). The sentence was fixed: `README.md:414` and `website/docs/web/components.md:135-138` now say ownership is a **teardown** guarantee, not a re-run guarantee, with the cleanup example. Auto-disposing effect-created streams and resources on re-run was rejected because an app may still hold a handle to one, and a silent auto-dispose is a worse failure than a documented duplicate. Coverage: two characterization tests in `tests/component-lifecycle.test.ts` pin both halves (no cleanup -> 2 watches and a doubled observer; cleanup -> 1). **Flagged honestly: those pass before and after by construction - they are contract coverage, not a regression witness.**

**Repro.** jsdom, real component.ts + real signals-core: a component with `addEffect([n], body)` whose body calls `__state/watch` on a second signal and returns no cleanup; write the dep signal twice, then write the watched signal once. Sema equivalent: `(effect (list @n) (fn () (watch other (fn (o v) …)) nil))`.

**Observed.** After two dep changes the component owns 3 watches (ctx.watchDisposers.size = 3) and a single `(put! other 99)` fires the observer three times: observed = [99, 99, 99]. Nothing is reported. The duplicates only disappear at unmount.

**Expected.** README:346-348 and website/docs/web/components.md:134-136 state "Anything an effect body creates — intervals, watches, streams, state — is owned by the component and disposed with it, so a returned cleanup only has to undo what the framework does not know about." That is true only for the teardown boundary, not for a re-run: for any effect with non-empty deps the framework's ownership does NOT prevent duplication, and omitting the cleanup silently multiplies subscriptions. Either scope effect-created resources to the slot (release them when the slot re-runs) or fix the sentence — as written it teaches the leak.

### 21. [MEDIUM] The "unnamed resource inside a component" guard is bypassed from an `effect` body, so the per-render runaway it exists to prevent still happens

**Status 2026-07-28 (harden phase): FIXED.** The unnamed-resource guard now keys off `getCurrentOwnerId(ctx)` rather than `ctx.renderContextStack.length`, so it fires anywhere a component owns the call - including an effect body, which `withOwnerContext` deliberately runs with an empty render stack. Witness: `tests/resource.test.ts` -> "refuses an unnamed resource anywhere a component owns the call", plus the real-WASM "refuses an unnamed resource created from an effect body" in `tests/resource-sema.test.ts`.

**Repro.** renderSema (real WASM): `(def tick (state 0))` + `(defcomponent view () (effect nil (fn () (resource (fn () "/api/tick")))) [:p (string-append "t" (number->string @tick))])`, mounted, then `(put! tick i)` for i in 1..4 with a flush between each.

**Observed.** 5 renders produce 5 fetches, ctx.resources.size === 5 and ctx.signals.size === 7, with errors === []. The guard at resource.ts:256 tests `ctx.renderContextStack.length > 0`, but runLifecycleSlot deliberately runs effect bodies under `withOwnerContext` only (render stack empty, owner stack set), so the check never fires. The message the guard would have printed — "an unnamed resource is recreated on every render" — is literally what happens. A component that re-renders on keystrokes would fire one request and allocate one live resource + signal + stream registration per keystroke, all only reaped at unmount.

**Expected.** The unnamed form should be rejected (or memoized by lifecycle slot index) anywhere an owner is active, not just when the render stack is non-empty. Note resource.test.ts:862 documents the effect-body case as intentional ("an effect body or an event handler"), so this may be a knowing trade — but the consequence contradicts the guard's own stated rationale, and "put the fetch in an effect" is the first thing a React-shaped user will write.

### 22. [MEDIUM] A focused `<select>` never receives new or removed `<option>`s

**Status 2026-07-28 (harden phase): FIXED.** A focused `<select>` gets a nested `childrenOnly` morph so its options are patched, and the user's selection is snapshotted and restored **by value** (morphdom drives `selectedIndex` off the `selected` attribute, which a user's own pick never sets). Witness: `tests/focus-preservation.test.ts` -> the "a focused `<select>`" block: "receives options the new render added", "loses options the new render removed", "keeps what the user picked across an option patch", "does not force a selection when the picked option is gone", "keeps every pick of a focused multi-select"; plus real Chrome in `e2e/tests/focus-preservation.spec.ts`.

**Repro.** Real Chrome, full Sema/WASM chain. `[:select {:id "s" :on-keydown "grow"} (map (fn (o) [:option {:value o} o]) @opts)]` with `opts` starting as `(list "a")` and `grow` setting it to `(list "a" "b" "c")`. `page.focus("#s")`, then press a key. Control: trigger the identical state change via a dispatched keydown while the select is NOT focused. Cause: `onBeforeElUpdated` (src/component.ts:353-365) returns `false` for a focused SELECT, and morphdom treats `false` as "skip this element AND its entire subtree" — the manual attribute copy above it has no child equivalent.

**Observed.** Focused: `<select id="s" data-sema-on-keydown="grow"><option value="a">a</option></select>` — 1 option. Unfocused control: 3 options. No error anywhere.

**Expected.** 3 options in both cases. Focus/caret preservation is about the element's own live state; its children are ordinary markup. A `<select>` whose options load asynchronously, or a dependent dropdown, silently never updates for a user who has the control focused — which is precisely the user who is about to pick from it.

### 23. [MEDIUM] Nested `on-mouseenter` handlers: only the innermost fires, so `mouseleave` arrives with no matching `mouseenter`

**Status 2026-07-28 (harden phase): FIXED.** The synthetic mouseenter/mouseleave path is a full ancestor walk (`walkSynthetic`) instead of a single `closest()`: every element actually entered runs its `mouseenter` outermost-first, every element left runs `mouseleave` innermost-first, with `relatedTarget` tested per element. Witness: `tests/event-modifiers.test.ts` -> "runs every ancestor being entered, outermost first", "runs every ancestor being left, innermost first", "enter and leave stay balanced across the whole crossing"; "does not re-enter an ancestor the pointer never left" is the control. Real pointer geometry in `e2e/tests/event-routing.spec.ts`.

**Repro.** Real Chrome with real pointer movement. `[:div {:id "card" :on-mouseenter "card-enter" :on-mouseleave "card-leave" :style "width:200px;height:100px"} [:button {:id "btn" :on-mouseenter "btn-enter" :style "width:200px;height:100px"} "buy"]]`. `page.mouse.move(5,5)` then move straight to the centre of #btn (one pointer move enters both card and button), then move back to (5,5). Cause: the synthetic mouseenter listener (src/component.ts:645-651) resolves the handler with `closest("[data-sema-on-mouseenter]")`, which finds only the NEAREST ancestor carrying the attribute, whereas real `mouseenter` fires on every element being entered.

**Observed.** After entering the button the hover log is `"btn-enter;"` — `card-enter` never ran. After leaving, the log is `"btn-enter;card-leave;"`: the card's `mouseleave` DOES fire (its `closest` lookup for `data-sema-on-mouseleave` reaches the card), so any app tracking hover with a boolean or counter now has a leave with no enter.

**Expected.** `card-enter;btn-enter;card-leave;` — the order observed when the pointer happens to cross the card's own padding first (I confirmed that path works). The enter/leave asymmetry is the damaging half: a hover-opened menu or tooltip gets a close it never got an open for. Nothing in website/docs/web/sip-markup.md mentions the nesting limitation — it documents only that `.capture` is a no-op on these two.

### 24. [MEDIUM] `.prevent.once` stops preventing once `.once` is spent, so the second submit navigates the page away

**Status 2026-07-28 (harden phase): FIXED.** `.prevent` is applied before `.once` is consulted, so a spent `.once` still prevents. Witness: `tests/event-modifiers.test.ts` -> ".prevent.once keeps preventing a submission it has already handled" and ".prevent.stop.once: the second dispatch still prevents, but stops nothing"; ".self.once still refuses to prevent for an event it filtered out" pins the other half of the rule. In real Chrome the pre-fix version navigated to `/navigated.html` - `e2e/tests/form-modifiers.spec.ts` asserts it no longer does.

**Repro.** Real Chrome, full Sema/WASM chain. `[:form {:action "/navigated.html" :method "get" :on-submit.prevent.once "save"} [:input {:type "hidden" :name "q" :value "1"}] [:button {:type "submit"} "Go"]]`. Click submit twice. Cause: the fixed modifier order in `tryRun` (src/component.ts:750-760) checks `.once` and `return`s before `.prevent` is applied.

**Observed.** First click: handler runs, counter 1, URL unchanged. Second click: the page navigates to `http://localhost:5173/navigated.html?q=1`. No error is reported.

**Expected.** This matches Vue (where `.once` removes the listener wholesale), so it may well be intended — but it is undocumented and the consequence is a full page navigation, i.e. the exact catastrophic outcome the modifier validation exists to prevent. website/docs/web/sip-markup.md documents only the `.self` case ("A handler that `.self` filtered out does not prevent the default and does not use up its `.once`") and describes `.prevent` unconditionally. Either the ordering should change or the docs should carry the same explicit warning `.self` gets.

### 25. [MEDIUM] Nothing gates the browser wasm artifact against current Rust source, so the JS suites can silently test a previously built VM

**Status 2026-07-28 (harden phase): FIXED.** `packages/sema-web/scripts/wasm-freshness.ts` writes a content fingerprint of the Rust inputs (plus the binary's mtime+size, and a hash of the hashing *rule*) to `packages/sema-wasm/pkg/.sema-web-source-stamp.json`, checked by the vitest setup and the Playwright `globalSetup`. It is self-bootstrapping - a changed binary re-adopts - so no build entry point has to remember to stamp and no CI path can go red on a correct run. Deliberately **not** the ledger's mtime gate: `actions/checkout` rewrites every `.rs` mtime, so an mtime gate would fail on every cache hit. Witness: `tests/wasm-freshness.test.ts` (11 tests against a throwaway tree), and the fix was proven in both harnesses by making a real `driver.rs` edit and by corrupting the stamp. Blind spot: the first run against an unstamped binary adopts it as-is; every Rust edit after that is caught.

**Repro.** git check-ignore -v packages/sema-wasm/pkg/sema_wasm_bg.wasm  (=> packages/sema-wasm/pkg/.gitignore:1:*), then: rg -n 'wasm' packages/sema-web/package.json packages/sema-web/playwright.config.ts packages/sema-web/vitest.config.ts

**Observed.** packages/sema-wasm/pkg/* is entirely gitignored, and the e2e fixtures load that binary directly (the failure stacks show http://localhost:5173/@fs/.../packages/sema-wasm/pkg/sema_wasm_bg.wasm). But no JS test entry point builds or freshness-checks it: package.json's "test" is bare `vitest run`, "test:e2e" is bare `playwright test`, and playwright.config.ts's webServer array only starts `vite e2e/fixtures` and `npx tsx e2e/mock-proxy.ts` — no wasm step, no pretest hook. Only jake's file recipe (jake/wasm.jake:18, deps crates/**/*.rs) does mtime checking, and nothing in the JS path invokes it. So editing a crate and running npm run test:e2e exercises whatever VM happened to be built last, and the suite reports green. This is the same class as the headline bug: a green suite that structurally cannot see the artifact under test. It is also precisely the trap I fell into this session — I wrongly concluded the wasm was stale, and only the cargo 'Finished release profile in 0.33s' line disproved it, because nothing else in the repo could tell me whether the binary matched the source.

**Expected.** The browser suites should depend on the wasm being current — e.g. a pretest/webServer step that runs the jake wasm.build file recipe (or an equivalent mtime/hash check) and fails loudly when packages/sema-wasm/pkg is older than crates/**/*.rs, so 'e2e green' is a statement about current source rather than about an unknown-vintage binary.

### 26. [MEDIUM] The public .d.ts leaks @preact/signals-core's ES2026 Disposable requirement into every consumer; skipLibCheck hides it in-repo

**Status 2026-07-28 (harden phase): FIXED.** `dist/index.d.ts` carries a `/// <reference lib="esnext.disposable" />` banner, so a consumer on a plain strict config no longer hits `TS2304: Cannot find name 'Disposable'` inside a signals-core `.d.ts` it does not own. Witness: `tests/package-boundary.test.ts` -> "declares the lib its public types depend on" and "type-checks under a strict config that does not skip lib checks", the latter spawning real `tsc` with `module: nodenext`, `strict`, **no** `skipLibCheck` - which the in-repo `tsc --noEmit` structurally cannot do, because `tsconfig.json` sets `skipLibCheck: true`.

**Repro.** Temp project with a node_modules symlink to packages/sema-web, app.ts = `import { SemaWeb } from "@sema-lang/sema-web";`, package.json {"type":"module"}, tsconfig {module:node16, moduleResolution:node16, target:es2022, strict:true, lib:["es2023","dom"]} and NO skipLibCheck. Run tsc 5.9.3.

**Observed.** error TS2304: Cannot find name 'Disposable' — raised inside node_modules/@preact/signals-core/dist/signals-core.d.ts(138,53), a file the consumer does not own and cannot edit. It is dragged in because dist/index.d.ts:2 is `import { Signal } from '@preact/signals-core';` — the package re-exports that type as part of its public surface. I isolated the toggle precisely: identical config + skipLibCheck:true => CLEAN; identical config + lib gaining "esnext.disposable" => CLEAN; without either => FAIL. The package cannot see this because its own tsconfig.json:11 sets skipLibCheck: true, so `npx tsc --noEmit -p tsconfig.json` exits 0 (I confirmed TSC=0). skipLibCheck defaults to false in tsc, so a consumer on a plain strict config hits this out of the box.

**Expected.** Consuming the package's types should not require the consumer to opt into skipLibCheck or add lib:"esnext.disposable". Either stop re-exporting the signals-core type from the public surface, or declare/document the required lib, or pin a signals-core version whose .d.ts does not need the Disposable global. At minimum the in-repo skipLibCheck should not be what makes the package look type-clean.

### 27. [MEDIUM] The shipped dist/ bundle has zero browser coverage: all 15 e2e fixtures import src/index.ts

**Status 2026-07-28 (harden phase): FIXED.** Two boundaries, because they are different boundaries. (a) `tests/package-boundary.test.ts` resolves the package by name through its own exports map from a throwaway project, with real `node` and real `tsc`. (b) `e2e/fixtures/dist-bundle.html` + `scripts/dist-bundle.sema` + `e2e/tests/dist-bundle.spec.ts` is the first browser fixture that imports `@sema-lang/sema-web` **by name** rather than `../../../src/index.ts`, so Chrome loads the built `dist/index.js`; the fixture touches state, SIP, components, delegated events and the router, because the failure it guards is a bundle missing a whole namespace while still booting. Both suites skip when `dist/` is absent, matching `testing-entry.test.ts`.

**Repro.** rg -n 'src/|dist/' packages/sema-web/e2e/fixtures/scripts/*.ts

**Observed.** Every one of the 15 fixture init scripts begins `import { SemaWeb } from "../../../src/index.ts";` (e.g. e2e/fixtures/scripts/init-counter.ts:1). Not one loads dist/index.js. So all 84 Playwright tests and all 846 vitest tests exercise vite-transformed pre-bundle source, and the only validation the published artifact gets is that tsup exits 0 — no test resolves the package through its own exports map or executes the bundle. I verified the bundle is not itself broken: a throwaway fixture importing ../../dist/index.js mounted counter.sema in real Chrome, rendered '0', and went to '1' on click. So this is a coverage gap, not a live break — but finding 1 (CJS entirely unreachable) is exactly the kind of packaging defect that only a test crossing the package boundary can catch, and it shipped.

**Expected.** At least one browser-level smoke test should load the built dist/ bundle through the package's real entry points (mirroring the crates.io-side scripts/test-packaged-sema-web.sh idea that AGENTS.md mandates for packaged behaviour), so bundle/exports/packaging regressions are caught rather than resting on 'tsup exited 0'.

### 28. [LOW] A bare route table whose key is `routes` is rejected, contradicting the invariant its own doc comment asserts

**Status 2026-07-28 (harden phase): FIXED.** `parseInitArg` disambiguates on the *value's shape* under `routes` (a handler-name string is the route `/routes`; a map is the options form) rather than on the key's presence. The doc comment's original justification was wrong twice over - measured against the real interpreter, a Sema map arrives with its keyword colons already stripped, so `:routes` and `"routes"` are indistinguishable at that point. Witness: `tests/router.test.ts` -> "registers a route whose pattern happens to be spelled 'routes'"; "still reads a map under 'routes' as the options form" is the control. Declared residual: `(router/init! {:routes "oops"})` now registers `/routes` instead of throwing, which leaves the app with no routes at all - visible on every URL through the route diagnostics, unlike a legal route being permanently unreachable.

**Repro.**   interp.getFunction("router/init!")!({ routes: "routes-page" });

i.e. Sema `(router/init! {"routes" "routes-page"})` — a bare `pattern -> handler` table registering the route `/routes`, spelled without the leading slash, which `normalizeRoutePath` (router.ts:135-144) explicitly documents as equivalent ("`todos` and `/todos` are the same route however each side spelled it").

**Observed.** Throws `Error: router/init!: :routes must be a map of pattern -> handler`. `parseInitArg` (router.ts:278) form-detects on `readOption(arg, "routes")` BEFORE any path normalization, so the table is misread as the options form and its handler string is rejected as a bad routes map.

**Expected.** Either register `/routes -> routes-page`, or fail with a message that names the real problem. The doc comment at router.ts:258-266 justifies the detection rule as "unambiguous rather than heuristic, because a bare table's keys are paths and a path is normalized to start with `/` — so no route can ever be named `routes` in the first place" — that reasoning is wrong, because normalization happens after detection and slash-less patterns are a supported spelling.

### 29. [LOW] `update!` in a render body melts down into ~100 renders and an opaque signals-core "Cycle detected" blamed on the component

**Status 2026-07-28 (harden phase): PARTIALLY FIXED.** The diagnosis is fixed; the wasted renders are not. `withRenderCycleHint` appends "this render wrote reactive state that it also reads. Move the `(put! ...)` / `(update! ...)` into an `(effect ...)`, an `(on-mount ...)`, or an event handler" to signals-core's bare "Cycle detected", keeping the original error object and its wasm frames (the only evidence of where the write happened). Witness: `tests/component-reentrancy.test.ts` -> "is reported with the cause named, not as a bare 'Cycle detected'"; "leaves an ordinary render error's message untouched" is the control. **Still open: the ~100 renders burned before signals-core's own cycle guard stops it.** Bounding them would need a render-write tripwire that fires before the cycle guard does.

**Repro.** `(defcomponent updater () (update! upd-render (fn (n) (+ n 1))) ... )` mounted, in Chromium with dev mode on. `update!` is `(put! ref (apply f (cons (deref ref) args)))` (src/reactive.ts:163) — the `deref` makes the written signal a dependency of the render effect, so the write re-triggers the render that performed it.

**Observed.** The counter reaches **101** and `onerror` fires once with context `component:updater` and detail `Eval error: JS callback error: JsValue(Error: Cycle detected ...)` plus a raw WASM stack trace. 101 full renders ran before @preact/signals-core's cycle guard stopped it. Nothing names the actual problem ("you wrote reactive state during a render") and nothing points at `update!`.

**Expected.** This is a known footgun — e2e/fixtures/scripts/composition.sema documents it — but the brief lists render as one of the positions `update!` should now be legal in, and the answer is "no, and the diagnosis is unusable". A dev-mode diagnostic along the lines of "component X wrote state it reads during render" would turn 101 wasted renders and a WASM stack trace into a one-line fix. Noting it because it is the only `update!` position that does not work, and because the failure is now a signals-core cycle rather than the VM trap the fixtures still warn about.

### 30. [LOW] The new gap-3 e2e fixture still documents the fixed WASM trap as live and keeps a workaround for it

**Status 2026-07-28 (harden phase): FIXED.** Same change as findings 17 and 35: `e2e/fixtures/scripts/lifecycle.sema` describes the trap in the past tense and its effect/interval path uses `update!`, so the fixture now *exercises* the restored contract in real Chrome rather than teaching a false rule about it. `e2e/tests/lifecycle.spec.ts` -> "an effect body may call a host function without trapping the VM" and "nothing traps the WASM VM across the whole lifecycle".

**Repro.** Read /Users/helge/code/sema/sema/packages/sema-web/e2e/fixtures/scripts/lifecycle.sema lines 5-8 and 20-26, and /Users/helge/code/sema/sema/packages/sema-web/e2e/tests/lifecycle.spec.ts lines 6-11. Then, in the real browser, `(defcomponent updater () (effect (list) (fn () (update! n (fn (v) (+ v 1))) (fn () (update! n (fn (v) (+ v 10)))))) [:div …])` mounted and unmounted.

**Observed.** lifecycle.sema:20-26 asserts in the present tense that "`apply` inside any host-invoked Sema call — a render, an on-mount callback, a watch, an effect body — traps the WASM VM (docs/limitations.md 38). Effect bodies are host-invoked, so they live under the same rule", and keeps `(define (bump-ticks) (put! ticks (+ @ticks 1)))` as a deliberate `update!`-avoiding workaround; lifecycle.spec.ts:6-11 likewise says the related shape "aborts the instance with RuntimeError: unreachable". docs/limitations.md:279 reads `### ~~38. Host-invoked Sema calls trap the WASM VM~~ -> RESOLVED (2026-07-28)`, and my probe proves it: `update!` inside an effect body and inside the cleanup it returns both work (n = 1 after mount, 11 after unmount), no errors, no "unreachable" in console or pageerror.

**Expected.** Per the standing constraint, a comment claiming the trap is live is stale and must be corrected, and the workaround should be dropped (src/component.ts:862-864 already says "now resolved" — the fixture contradicts it). This one is worse than a stale comment: it is a fixture that teaches every future reader a false rule about `update!` in effect bodies.

### 31. [LOW] A body-decode failure discards the HTTP status even though it is in scope

**Status 2026-07-28 (harden phase): FIXED.** `response.status` is bound at the fetch and read by the outer catch, so a 200 whose body will not decode reports status 200 rather than null. Witness: `tests/resource.test.ts` -> "reports a malformed JSON body without clobbering the previous value"; the browser equivalent (a 200 whose body will not decode keeping its status) is in `e2e/tests/resource.spec.ts`.

**Repro.** Resolve a resource's fetch with `new Response("not json", {status: 200, headers: {"content-type": "application/json"}})` and read the signal.

**Observed.** `{loading:false, value:null, error:"resource: response was not valid JSON", status:null}` — status is null for a response that was demonstrably 200. resource.ts:333 hardcodes `fail(mySeq, e, null)` in the outer catch, discarding `response.status`, which is bound at that point for every failure raised after the fetch resolved (decode failures, an aborted-but-unreported body read).

**Expected.** `(:status @r)` should read 200 for a 200 whose body failed to decode, the same way it reads 500 for an error status. As written, a component that branches on `(:status @r)` cannot distinguish "transport failed before any response" from "server answered 200 with garbage".

### 32. [LOW] A throwing error reporter turns a resource failure into an unhandled promise rejection and loses the report entirely

**Status 2026-07-28 (harden phase): FIXED.** The `ctx.onerror` call is contained (`reportContained`) and the queued attempt carries a `.catch`, so a throwing reporter can no longer turn a resource failure into an unattributed window-level unhandled rejection. Witness: `tests/resource.test.ts` -> "contains a throwing error reporter instead of leaking an unhandled rejection", which reproduces the ledger's exact `unhandledRejection: Error: reporter exploded`.

**Repro.** Install `ctx.onerror = () => { throw new Error("reporter exploded"); }`, create a resource whose spec returns an unusable value (e.g. the number 42), flush, and listen for process/window unhandledRejection.

**Observed.** `unhandledRejection: Error: reporter exploded`. `fail()` calls ctx.onerror as its last statement and is itself invoked from inside runAttempt's catch blocks, so a throw there escapes the async function; `scheduleAttempt` fires it with a bare `void runAttempt(mySeq, myController)` and no `.catch`. In a browser this surfaces as a window-level unhandled rejection with no attribution to the resource, and the original failure is never reported anywhere.

**Expected.** Every other ctx.onerror call site in this package sits on a synchronous stack where a throwing handler surfaces at its origin; the detached-microtask path should not be the one that degrades to an unattributed unhandled rejection. Wrapping the onerror call (or attaching a .catch to the queued attempt) would contain it.

### 33. [LOW] Gap 4 is the only closed gap with no real-browser coverage and no user documentation

**Status 2026-07-28 (harden phase): FIXED.** Browser coverage: `e2e/fixtures/resource.html` + `scripts/resource.sema` + `scripts/init-resource.ts` + `e2e/tests/resource.spec.ts` - 7 tests in real Chrome with real WASM, real fetch and `page.route` recording which URLs actually left the page (the only way to tell "refetched" from "re-rendered the old response"): initial load, state-driven refetch, no-refetch on an unchanged request, `refresh!` from a delegated handler, a 500 keeping the previous value, a 200 whose body will not decode keeping its status, and unmount draining every registry to 0 with no pageerror. Docs: new `website/docs/web/resources.md` (linked from the sidebar) and a `resource` section in `packages/sema-web/README.md`.

**Repro.** `ls e2e/tests` and `grep -rn resource e2e/tests/*.spec.ts`; `grep -rn resource website/docs/web/*.md`.

**Observed.** e2e/tests has specs for composition, lifecycle, form-modifiers, router, store-persistence, sip errors, focus, multi-instance, script loading — and none for `resource`. website/docs/web has no resources page; the only mentions of `resource` in the site docs are incidental examples inside testing.md. All resource coverage is jsdom (resource.test.ts) plus jsdom+WASM (resource-sema.test.ts).

**Expected.** Given the wave's own lesson that a green unit suite is not evidence about the browser, the one shipped primitive whose entire job is to touch the network deserves a browser spec. My throwaway browser probe found the finding-1 collision reproduces identically in Chrome, so jsdom is not currently hiding a *different* bug — but nothing in CI would notice if it started to.

### 34. [LOW] `.once` state migrates to the wrong row in an unkeyed list

**Status 2026-07-28 (harden phase, reclassified 2026-07-29): CLOSED BY DECISION - deliberate non-change, diagnosed and documented.** The behaviour is unchanged **on purpose**. The ledger's Expected is "either both reordered rows fire or neither does"; the only way to make both fire is to clear the spent mark when morphdom patches the element, and that destroys what `.once` is for - `[:button {:on-click.once "save"} (if @saving "Saving..." "Save")]` would re-arm the moment its label changes, so a double-click submits twice. That is a worse defect than the one being fixed and it would invert two existing contracts. Item identity genuinely is not recoverable from an unkeyed list; only a `:key` supplies it. What shipped instead: a dev-mode diagnostic `sip-render:once-without-key:<parent>` on exactly the hazardous shape (an unkeyed element declaring `.once` with unkeyed same-tag siblings), live at `packages/sema-web/src/sip.ts:389`, plus the `.once` bullet in `website/docs/web/sip-markup.md`. Witness: `tests/sip-keys.test.ts` -> the ".once on unkeyed siblings" block (8 tests, incl. "says nothing once the rows carry a :key" and "reports once per parent, not once per row") and a characterization test in `tests/event-modifiers.test.ts` reproducing the ledger's exact migration.

**Repro.** jsdom with the real morphdom and real signals. `[:ul (map (fn (r) [:li {:on-click.once "h"} r]) @rows)]` with `rows` = `("a" "b")`. Click row 0 ("a"), then set `rows` to `("b" "a")`, then click both rows. Cause: `firedOnce` is a `WeakMap<Element, Set<string>>` keyed by node identity (src/component.ts:612), and morphdom reuses the position-0 node for a different item when the list is unkeyed.

**Observed.** Handler fires for "a", then after the reorder only one more fire occurs and its `ev.target.textContent` is "a" again. Row "b" — which the user never clicked — is permanently dead, and row "a" — already spent — fires a second time.

**Expected.** Either both reordered rows fire (fresh identities) or neither does. website/docs/web/sip-markup.md documents only the harmless direction of this ("If a re-render replaces the element ... the new element starts fresh"); the dangerous direction, a reused element making the wrong item's `.once` dead, is not mentioned in the `.once` bullet even though the keys section warns about unkeyed lists generally.

### 35. [LOW] Fixture comments still assert the WASM `unreachable` trap is live, steering code away from now-legal constructs

**Status 2026-07-28 (harden phase): FIXED.** `e2e/fixtures/scripts/forms.sema` now reads "It could not until 2026-07-28 (the WASM trap recorded as docs/limitations.md 38, now RESOLVED)"; `lifecycle.sema` and `lifecycle.spec.ts` were corrected under finding 17. Re-verified in this pass by grepping `src/`, `tests/`, `e2e/`, `README.md` and `website/docs/web/` for present-tense trap claims: none remain. The only surviving present-tense sentence is `e2e/tests/form-modifiers.spec.ts:173`, which explains that *if* a trap occurred it would surface as a page error rather than an assertion failure - a rationale for the guard, not a claim that the defect is live.

**Repro.** `docs/limitations.md:279` reads `### ~~38. Host-invoked Sema calls trap the WASM VM~~ → RESOLVED (2026-07-28)`. Three sema-web files still state it in the present tense: packages/sema-web/e2e/fixtures/scripts/forms.sema:9-14 ("a host call inside a mounted render traps the WASM VM (see docs/limitations.md) ... which is the shape that traps for host calls"); packages/sema-web/e2e/fixtures/scripts/lifecycle.sema:5-8 and 20-24 ("`apply` inside any host-invoked Sema call ... traps the WASM VM (docs/limitations.md 38). Effect bodies are host-invoked, so they live under the same rule the render already does."); packages/sema-web/e2e/tests/lifecycle.spec.ts:7-9 ("a host call inside a `map` callback inside a mounted render — aborts the instance with 'RuntimeError: unreachable'").

**Observed.** All three read as live constraints on a resolved defect. lifecycle.sema:20-24 goes further and prescribes a workaround ("`put!` with an explicit read, not `update!`"), so the stale claim is actively shaping the fixture's code.

**Expected.** Past tense, or removal. `src/component.ts:860-864` already gets this right — it explains the guard's one-day absence and states the defect is resolved. The fixtures should match. A future agent reading forms.sema will design around a trap that no longer exists.

### 36. [LOW] Last remaining side-effect inside a debug_assert! in the workspace: a state-clearing .replace(None) that release builds strip

**Status 2026-07-28 (harden phase): FIXED.** `crates/sema-wasm/src/driver.rs:155` is now `let stale_stop = driver.debug_stop_requested.replace(None);` with the `debug_assert!` observing the binding - the shape `restricted.rs` was corrected to, and matching the unconditional sibling on the next line. The ledger's second half is done too: `crates/sema-core/tests/debug_assert_purity.rs` is a workspace-wide lint that walks every `.rs` under `crates/`, masks comments and string / char / raw-string literals (so it cannot trip over its own vocabulary), delimiter-matches each `debug_assert!`/`_eq!`/`_ne!` body and rejects a closed list of 30 mutator calls; it asserts it actually scanned (>100 files, >20 macro sites) so it cannot pass by finding nothing. The narrow `restricted.rs` source-grep is kept as well.

**Repro.** Read crates/sema-wasm/src/driver.rs:155. Mechanism demo: rustc -O -C debug-assertions=off vs =on on `let flag=Cell::new(Some(7u32)); debug_assert!(flag.replace(None).is_none()); println!("{:?}", flag.get());`

**Observed.** crates/sema-wasm/src/driver.rs:155 is `debug_assert!(driver.debug_stop_requested.replace(None).is_none());` inside PromiseDebugDrive::begin. Release strips the entire macro, so the .replace(None) — which is doing double duty as both the assertion and the clear of stale re-entrancy state — never runs in the builds wasm-pack produces. My rustc probe shows the divergence directly: debug-assertions=off prints `flag = Some(7)` (stale value survives), =on panics. The smell is the asymmetry with its own sibling: the very next line, driver.rs:156, does the same 'clear stale pending debug state at begin' job for debug_breakpoints_pending as a REAL statement, unconditionally. I could NOT construct a reachable stale state, so I am not claiming a live bug: the only setter (driver.rs:593, in stop_debug's no-session branch) fires only while a drive is in flight, and both exit paths clear the flag (finish() replace(None) at :181, Drop set(None) at :190), while settle_debug_action normalises debug_root to None at :1091-1093. Note the existing regression guard at crates/sema-vm/src/restricted.rs:1733 is a source-grep hard-coded to restricted.rs's own text, so it structurally cannot see this second instance.

**Expected.** The pop/replace should be its own unconditional statement with the debug_assert! observing the result — exactly the shape restricted.rs:296-297 was corrected to (`let popped = ...; debug_assert_eq!(popped, ...)`). Ideally the restricted.rs source-grep guard is generalised into a workspace-wide lint so this class cannot reappear in a third place.

### 37. [LOW] RuntimeError: unreachable observed once in real Chrome after the fix — NOT reproducible, recorded for the record only

**Status 2026-07-28 (harden phase): FIXED as far as evidence allows - see the caveat.** `e2e/tests/reentrancy.spec.ts` now drives the exact scenario permanently (force-render! from an effect body, then unmount, then a state write) in real Chrome and asserts no "unreachable" and no "Maximum call stack size exceeded" across the whole sequence: "nothing traps the WASM VM across the whole re-entrancy sequence". It has never reproduced the trap. **This does not prove no second `unreachable!` site exists** - it proves this path no longer reaches one, and the case is a standing spec rather than a deleted probe.

**Repro.** NO LONGER REPRODUCIBLE — the triggering spec was deleted by a concurrent agent mid-session. Original: cd packages/sema-web && PLAYWRIGHT_CHROMIUM_EXECUTABLE="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" npx playwright test --reporter=line, failure at e2e/tests/zzadv.spec.ts:49 'force-render! from inside an effect body', on the `(component/unmount! "#c")` step (spec line 72) for a component whose effect body called component/force-render! on itself.

**Observed.** page.evaluate threw `RuntimeError: unreachable` with a pure-wasm stack (sema_wasm_bg.wasm wasm-function[2911] / [3359] / [3404] / [666] / [134] ...) via SemaInterpreter.evalGlobal. I am flagging this ONLY because constraint 1 declares this trap resolved as of today (docs/limitations.md section 38) and something still produced it in a real browser. I must be explicit about what I could NOT establish: (a) my stale-wasm explanation is retracted — the rebuild that made it disappear was a cargo no-op ('Finished release profile in 0.33s'), so the binary already had the restricted.rs fix; (b) a concurrent agent deleted e2e/tests/zzadv.spec.ts and its fixtures minutes later, so I cannot re-run it and cannot rule out that they were editing the fixture between my two runs; (c) the first run was 6-way parallel and the re-run was --workers=1, so ordering differs. On the re-run that same test failed on a different assertion (zombie re-renders continuing after unmount: 515 vs 344) with no trap. The restricted.rs fix addressed one specific unreachable! guard; other unreachable! sites exist and could be reached by a different path.

**Expected.** No wasm-level `unreachable` trap should ever reach JS. Someone should deliberately re-derive the force-render!-from-inside-an-effect-body-then-unmount case as a permanent e2e test (it was a throwaway probe) to establish whether a second unreachable! site is still live, rather than trusting a single unreproduced sighting either way.

## What each angle attempted

Recorded so that an absence of findings can be judged, not merely trusted.

### packages/sema-web gap 5 (src/router.ts — :query, not-found, router/link) and gap 10 (src/testing.ts — renderSe

VERIFIED BASELINE FIRST: `npm test` = 846 passed / 30 files, exit 0; `npx tsc --noEmit` TSC=0. I did NOT re-run `npm run build` (its `rm -rf dist` would race a parallel agent's `testing-entry.test.ts`, which reads dist/); I inspected the existing 09:47 dist output instead, and tsc-clean covers the compile signal.

IMPORTANT — parallel agent in the same tree: `tests/zprobe-adv-lifecycle.test.ts`, `tests/zzq4res-probe.test.ts`, `tests/zzq4sema-probe.test.ts` and several `e2e/**/zz*` files appeared at 09:51-09:59 during my session and are NOT mine. All 23 current suite failures live in those three files (gap 3/gap 4 territory). I left them untouched per the no-clobber rule. My own five probe files (tests/zzprobe{,2,3,4,5}.test.ts) and one scratchpad browser script are deleted; `grep zzprobe tests/` is empty.

HOW I PROBED: five throwaway vitest files (16 + 13 + 11 + 6 + 2 probes) against both the mock interpreter and the REAL WASM interpreter via renderSema, plus a real-Chrome playwright script (`PLAYWRIGHT_CHROMIUM_EXECUTABLE=/Applications/Google Chrome.app/...`) to prove the two URL-level findings are browser behaviour and not jsdom artifacts.

ROUTER — WHAT BROKE (see findings): literal-segment percent-encoding; the `/\host` bypass of EXTERNAL_PATH_RE (+ uncaught SecurityError out of the delegated click handler); the `{"routes" "h"}` bare-table rejection.

ROUTER — WHAT DID **NOT** BREAK (this is how you judge the above):
- Malformed %-escapes: `?a=%`, `?%=x`, `?%E0%A4=v` all keep raw text, never throw. Malformed param escapes likewise.
- Duplicate/empty/valueless keys: `a=1&a=2&a=3` -> list; `a` and `a=` -> ""; `=x` and `&&&` dropped; `a=b=c=d` -> value keeps the `=`; `+a+=+b+` -> `" a "/" b "`; double-encoding `%2526` -> `%26` (single decode, correct).
- Prototype safety: query map is null-prototype, `?__proto__=x&__proto__=y` lands as a real own key holding a list.
- Query keys reaching Sema through the real interpreter: `{:tab "open" :tag ("a" "b")}`, params decoded, `(:tab ...)` works, zero errors. Keys with a pre-existing colon (`?%3Atab=y`) become `::tab` — a DISTINCT keyword, so my "silent collision with `?tab=`" hypothesis is FALSE (count = 3, nothing overwritten). Keys with a space/`%`/NUL produce unreadable keywords (`:a b`) but the values remain reachable via `get`, so no data loss.
- Re-init listener accounting is exact: 3 inits (hash -> history -> hash) = 3 window adds / 2 removes / 3 doc adds / 2 doc removes, cleanupHooks stays 1. The teardown closure correctly captures the OLD event name. NO leak.
- Re-init atomicity holds: `(router/init! {"/a" "ha" "/b" 42})` throws and leaves the previously-working `/a` route intact; mode/not-found/scroll/focus are all assigned after the compile loop.
- not-found: never inherits params from a previous match (`/todos/7` -> `/nope` gives `:params {}`), and route diagnostics record correctly in every case I tried, including `"/nope/1?x=1 -> (no match) -> nf"` and `"/s%C3%B8k -> (no match) -> not-found-page"`.
- router/link hostile input: non-string/array/map/NaN paths all degrade to `<span>` with a reported error; a caller `href` or the marker attr is refused loudly; an unusable attribute name (`""`, `"a b"`, `"x<y"`) is dropped by SIP with a reported InvalidCharacterError and does NOT take down the render; `:__proto__` in attrs is silently dropped, not a pollution vector.
- ReDoS: `/f/:a:b:c:d:e:f` (six adjacent params) against a forced-fail path stays ~1ms at n=26; no catastrophic backtracking. escapeRegexLiteral covers every metachar, so no regex injection from literal segments.
- Navigation during a render works and does not loop: a render body calling `(router/push! "/b")` produces `<div>/b</div>`, one render diagnostic, two route diagnostics, zero errors.
- Perf: 200KB query value = 953ms, 20k duplicate keys = 515ms, 5000 distinct keys = 438ms — the cost is jsdom's URL machinery, not parseQuery. Not a router defect. (Side note, not reported: dev diagnostics retain the FULL raw URL in `detail` — I measured a 200,010-char entry — so the ring is bounded in entry count but not in bytes.)

TESTING UTILITY — WHAT DID **NOT** BREAK:
- The "no test-only deps in the production bundle" claim HOLDS. `dist/index.js` and `dist/index.cjs` contain zero node-builtin imports and zero occurrences of `renderSema`; `dist/testing.js` is 7,292 bytes, reaches the runtime only through `import("./index.js")`, and contains none of the runtime's marker strings. (One cosmetic note: esbuild emits the specifiers as bare `"fs"`/`"module"`/`"path"`, stripping the `node:` prefix the source wrote. Node-equivalent; the existing guard regex tolerates it deliberately.)
- Two live, non-disposed screens do NOT cross-talk: clicking screen 2's button leaves screen 1's signal at 0, no errors on either side.
- `dispose()` is genuinely thorough and idempotent: after it, `leaks()` is `{}`, the container is emptied, clicking a detached node throws nothing, a second `dispose()` is a no-op.
- `renderSema` failing on a bad target disposes its own instance and leaves an earlier live screen usable.
- `mount()` on an already-mounted target does not double-register anything; `leaks()` after unmount is `{}`.
- `LeakCounts` has no blind spot I could find: `resourcesByKey` is pruned by the resource finalizer, `signalFinalizers` by `disposeSignal`, and every `ctx.signals.delete` goes through `disposeSignal`.
- Marginal, not reported: `screen.flush()` never settles under `vi.useFakeTimers()` (it awaits `setTimeout(0)`); and a render that calls `router/init!` after the baseline makes `leaks()` report a permanent false-positive `{cleanupHooks: 1}` — both contrived.

EXISTING COVERAGE GAP that let findings 1 and 2 through: `tests/router.test.ts` tests `//evil.example`, `https://`, and `javascript:` but never a backslash; non-ASCII appears only once, as a query VALUE ("São Paulo"), never as a route pattern. `e2e/tests/router.spec.ts` has neither.

### @sema-lang/sema-web (/Users/helge/code/sema/sema/packages/sema-web) — adversarial cross-feature pass in a real

BASELINE VERIFIED MYSELF: `npm test` → 846 passed / 30 files, exit 0. `npx tsc --noEmit` → TSC=0. Playwright full suite → **79 passed**, exit 0 (the brief said 67; it is 79 now). `packages/sema-wasm/pkg` is a fresh post-fix build (Jul 28 08:12), and the section-38 fix is present but UNCOMMITTED in `crates/sema-vm/src/restricted.rs`.

METHOD: I wrote 8 successive throwaway browser fixtures (`e2e/fixtures/zzprobe.{html,sema}` + init + spec), drove each against real WASM + real morphdom + real events, read `web.context` registries and the diagnostics timeline directly out of the page, and deleted all of my files afterwards. No src file was modified.

WHAT BROKE (all confirmed with my own eyes, details in findings): per-row `effect` teardown hits the wrong row in a keyed list and orphans a live `setInterval`; a `resource` never refetches when its spec's inputs change; a named `resource` inside a list/route child collapses to one shared instance and leaks past navigation; `local` inside a list child is one shared cell whose per-row entries are never released; `watch`/`computed` called from a render body accumulate one live registration per render.

WHAT DID **NOT** BREAK — this is the part that makes the above meaningful:
- Section 38 is genuinely dead. Inside a *mounted* render I ran Sema `try`/`catch`, a host-native call (`dom/query`) inside a `map` callback, and `apply` inside a `map` callback, all in one render. All three returned correct values; zero `unreachable`, zero pageerrors.
- `update!` works in on-mount, effect body, watch callback, and event handler — all incremented exactly once.
- A throwing child **inside a `map` callback** is correctly isolated: siblings render, the surviving rows are intact, the error lands as `component:kid` (the CHILD, not the mounted root), and the app recovers when the child stops throwing. Per-child isolation is real.
- `web.dispose()` with a resource in flight + an interval + 13 watches + duplicate keys + dev mode on: every registry (`resources`, `resourcesByKey`, `streams`, `signals`, `intervals`, `watchDisposers`, `handles`, `listeners`, `cleanupHooks`, `signalFinalizers`) drained to 0, `#app` emptied, and releasing the pending fetch *after* dispose produced no pageerror and no "Unknown callback handle". Clean.
- Keyed-list DOM identity holds (expando probes survive reorder/insert/remove). The dev duplicate-key detector fires on a real duplicate and produced zero false positives on unique keys.
- Router across real WASM: `:params`, `:query` (incl. repeated key → list), not-found, `router/link`, focus/scroll effects, back button — all correct.
- Props containing a resource handle marshal fine (plain number); nested prop maps read correctly two levels deep.

NOT COVERED: LLM proxy hardening, SSE/WebSocket, `history` mode, `renderSema`/testing.ts, and the dev overlay were only exercised incidentally. I did not run the full Playwright suite again after my probes, to avoid disturbing parallel agents' dev servers.

CO-TENANCY NOTE (important for the orchestrator): other agents are working in this tree concurrently. `tests/zprobe-adv-lifecycle.test.ts`, `tests/zzq4res-probe.test.ts`, `tests/zzq4sema-probe.test.ts` (19 failures) and `e2e/{fixtures,tests}/zzadv.*` are **theirs, not mine** — I left them untouched. Additionally, after I deleted my own probes at ~10:00, another agent independently created files at the same paths (`e2e/.../zzprobe.*`, timestamps 10:01–10:02, different content); those on disk now are theirs. Excluding all `z*probe*`/`zzadv*` files, the pre-existing 30-file / 846-test suite still passes and tsc is clean.

### Gap 3 (component lifecycle: effect / on-unmount) in /Users/helge/code/sema/sema/packages/sema-web — src/compon

BASELINE VERIFIED FIRST: `npm test` = 846 tests / 30 files pass (exit 0), `npx tsc --noEmit` exit 0, `npm run build` exit 0, `npx playwright test e2e/tests/lifecycle.spec.ts` 6 passed with the real WASM runtime.

HOW I PROBED. Two harnesses. (a) A throwaway vitest file (tests/zprobe-adv-lifecycle.test.ts) using the REAL component.ts + real @preact/signals-core + real morphdom + the shared mock interpreter, i.e. the same setup as tests/component-lifecycle.test.ts. (b) A throwaway Playwright fixture (e2e/fixtures/zzadv.html + scripts/zzadv.sema + scripts/init-zzadv.ts + e2e/tests/zzadv.spec.ts) driving the REAL WASM interpreter in real Chrome, because per constraint 2 a green jsdom suite is not evidence about the browser. Every finding below marked confirmed was reproduced in harness (b) except finding 6 (jsdom only, pure JS bookkeeping). I also read crates/sema-wasm/src/lib.rs (allocate_callback_handle / release_callback) and packages/sema/src/index.ts (_wrapCallbackHandle) to establish the real callback-handle semantics before trusting any identity comparison.

ALL THROWAWAY FILES DELETED. Re-verified after cleanup: tsc exit 0, build exit 0, `npx vitest run tests/component-lifecycle.test.ts tests/component.test.ts tests/component-props.test.ts tests/context.test.ts` = 102 passed, real `e2e/tests/lifecycle.spec.ts` = 6 passed. (A full `npm test` now shows 12 failures in tests/zzq4res-probe.test.ts and tests/zzq4sema-probe.test.ts — those are another agent's in-flight probe files, present in the tree but not mine; I left them and their test-results/ dirs untouched.)

WHAT I TRIED THAT DID **NOT** BREAK — this is the part that tells you how much the absence of further findings is worth:
1. Handle double-release on the deps-equal path (`if (next.fn !== current.fn) releaseCallback(next.fn)`). I expected a dangling-handle bug because wasm shares one handle per identical Sema value with NO refcount. It is actually sound: packages/sema/src/index.ts memoizes one JS wrapper per handle, so same Sema value => identical JS object => no release. Verified with a bridge-accurate mock (handle table + wrapper cache copied from the real bridge): no error.
2. Shared *effect body* across two instances. Broke in the bridge-accurate mock, but in the real browser it self-heals: an effect body is re-marshalled on every render, so a released handle is replaced by a fresh one before the next invocation. Log came out "ran;ran;ran;", zero errors. That is why finding 2 is scoped to callbacks that are *stored and invoked later* (on-unmount hooks, effect cleanups) — I did not report the effect-body variant.
3. Pruning a slot that shares a handle with a live sibling *inside one component*: fails in the mock, heals in the browser for the same re-marshal reason. Not reported.
4. Infinite render loop: `(effect nil (fn () (put! n (+ @n 1))))` on a signal the render reads. Does NOT hang — signals-core's batchIteration guard fires after ~100 rounds; observed 102 renders then "Cycle detected" reported through onerror as `effect:v#0`. Noisy (100 morphdom patches) but bounded and diagnosable, so not reported.
5. Throwing effect body, throwing effect cleanup, throwing on-unmount hook, throwing render after live effects exist, bad deps types: all reported with correct `effect:v#N` / `effect-cleanup:v#N` / `on-unmount:v#N` context, siblings still run, teardown completes, pendingLifecycle drained, callbacks released exactly once. Nothing to add.
6. Double unmount, remount over the same selector, dispose() with live effects, `disposeAllComponents` twice: each cleanup ran exactly once.
7. `untracked` around flushLifecycle really does stop an effect-body read from subscribing the render.
8. On-mount cleanup that writes a signal: `destroyMountedComponent` runs `mountCleanup` (component.ts:486) BEFORE `component.dispose()` (496), so that write triggers one extra full render + morphdom patch of the component being destroyed — an ordering asymmetry with the deliberate comment at component.ts:505 ("After the render effect is stopped, deliberately"). I confirmed the extra render happens (renders = ["render:0","render:1"]) but found no downstream corruption: the re-registered slots are still drained by disposeLifecycleSlots, and intervals/watches/signals are cleared later in the sequence. Judged benign; not reported as a defect, but it is the one place where the file's own stated invariant is not applied uniformly.
9. VM-trap regression check (constraint 1): `update!` inside an effect body AND inside the cleanup it returns works in the real browser (n=1 after mount, 11 after unmount, no errors, no "unreachable"), as does `try`/`map?` via component/render. Nothing here needs a workaround.

### Gap 4 — /Users/helge/code/sema/sema/packages/sema-web/src/resource.ts (async resources: `resource`, `resource/

Read src/resource.ts line by line against src/context.ts (owner stack, signal finalizers, stream registry, disposeContextResources), src/component.ts (withComponentContext vs withOwnerContext, runLifecycleSlot, destroyMountedComponent, renderComponent, COMPONENT_SEMA_PRELUDE / component/render), src/callbacks.ts, src/reactive.ts (watch/computed), and the gap-4 section of docs/plans/archive/2026-07-02-sema-web-framework-gaps.md. Baseline verified first: `npm test` = 846 passed / 30 files, exit 0.

Wrote and RAN four throwaway probe suites (all deleted afterwards; no source file was edited):
1. tests/zzq4res-probe.test.ts — 17 mock-interpreter probes.
2. tests/zzq4sema-probe.test.ts — 5 probes through the REAL WASM interpreter via renderSema (real defcomponent/component/render/keyed lists/signals/morphdom).
3. tests/zzq4chaos-probe.test.ts — 120 seeded randomized interleavings of refresh / cancel / out-of-order settle / reject / probabilistic abort-honouring, asserting a monotonic "never apply an older response" invariant plus zero unhandled rejections.
4. tests/zzq4throw-probe.test.ts — a render that throws while a resource settles.
Plus a real-browser probe (e2e/fixtures/zzq4res.html + .sema + init + e2e/tests/zzq4res.spec.ts) driven with `PLAYWRIGHT_CHROMIUM_EXECUTABLE=/Applications/Google Chrome.app/... npx playwright test`, using page.route to record which URLs actually left the browser. All probe files and the playwright test-results dir for my run were deleted; `git status` for packages/sema-web now shows only other agents' pre-existing zzprobe* files, and tests/resource.test.ts + tests/resource-sema.test.ts still pass (55 tests).

WHAT DID NOT BREAK (this is the part that makes the finding list meaningful):
- The sequence-number staleness guard is sound. 120 randomized interleavings found no case where an older response overwrote a newer one and no unhandled rejection. Cancel-then-refresh with the pre-cancel response resolving LAST is handled. A 51-deep refresh storm settled in reverse order ends on r50. 20 synchronous refreshes coalesce to exactly one fetch. Every `await` in runAttempt is followed by a seq re-check, including the body decode and the error-body read.
- Abort during settle is clean: unmount / cancel / dispose all bump seq before aborting, so a controller handed to runAttempt is never pre-aborted at a live seq; a deliberate abort never reaches onerror.
- Unmount between dispatch and settle: the spec is never invoked (handle already released), the signal is disposed, a late-resolving response writes nothing and reports nothing.
- Teardown drains: destroyMountedComponent (cleanupStream + disposeSignal -> finalizer) and disposeContextResources both leave ctx.resources / ctx.resourcesByKey / ctx.streams at 0 and release the spec handle exactly once. The handle-dedup branch in replaceSpec behaves as its comment claims.
- A spec that throws synchronously, returns nothing usable, or returns a Promise is reported through the signal and onerror without clobbering the previous value; a spec throwing on a *refresh* keeps the prior value.
- `resource/refresh!` from a `watch` callback does not cycle (the watch's prev!==current guard breaks the loop).
- Two resources cannot share a signal id: ctx.nextSignalId is only ever `++`'d, never assigned.
- A render that throws while a resource settles does NOT corrupt state or produce an unhandled rejection — renderSip catches invalid tags itself (reported as `sip-render:invalid-tag:…`).
- Keyword *values* keeping their colon inside `:headers` / `:body` is a package-wide convention (http.ts and llm.ts strip keys only), not a resource.ts defect, so I am not reporting it.

NOT ATTEMPTED: concurrency against a real network/HTTP2 stack; memory-pressure or GC interaction with released WASM callback handles; Safari/Firefox.

### Gap 6 (event modifiers in src/sip.ts, form helpers in src/dom.ts, EventDelegator in src/component.ts) of /User

Baseline verified myself first: `npm test` = 846 tests / 30 files, exit 0.

Two throwaway jsdom probe suites (36 scenarios) plus four throwaway real-browser Playwright fixtures driving the full Sema+WASM chain in Chrome (`PLAYWRIGHT_CHROMIUM_EXECUTABLE=.../Google Chrome`). Every finding below was reproduced in real Chrome, not only jsdom — deliberately, per the "green unit suite is not evidence about the browser" constraint. All throwaway files deleted; `git status` and `npm test` are byte-identical to the baseline afterwards. I did not fix anything.

WHAT I TRIED THAT DID NOT BREAK (so the absence of findings there means something):
- Modifier parsing: `.stop.stop` dedupes; `:on-click.` / `:on-.prevent` / `on-` / `.Prevent` / `.prevnt` are all loud errors under `sip-render:on-handler`; `.stop.prevent` and `.prevent.stop` do encode byte-identically; a non-string handler value errors; a value failing SEMA_IDENT_RE is rejected and leaves no orphan mods attribute.
- Capture semantics: `.capture` ancestor runs before a native descendant listener; nested `.capture` runs outer-then-inner; a child's `.capture` runs before an ancestor's plain handler; a plain handler does not double-fire when `ctx.captureEvents` is primed; `.capture.stop` correctly suppresses descendants.
- `.self`: filters a nested target, does not burn `.once` when filtered (`.self.once` confirmed), correct on the element itself.
- `.once`: survives an attribute-only re-render on a preserved node (real morphdom).
- Form helpers: disabled and unnamed controls excluded, readonly included, unchecked checkbox excluded, multi-select yields a list, duplicates yield string→array→push correctly, a `__proto__` field lands as an ordinary key with no prototype pollution, a File and a string under one name mix into one array, the `FormData(form, submitter)` submitter path really works under jsdom 29 (not silently the fallback), a form nested in a form resolves each control to its own `.form`, `resolveForm` handles `form="<id>"`.
- Non-element targets: `mouseover` dispatched on a Text node and on the mount root does not throw.
- `resolveForm` with `form="nonexistent"` returns the ancestor form, but `FormData` then correctly omits the unassociated control.

CONFIRMED BUT JUDGED TOO LOW-VALUE TO LIST SEPARATELY:
- `(ev as any).__sema_stop` is a custom property that `dispatchEvent` never clears, so re-dispatching the *same* Event object after a handler called `dom/stop-propagation!` once suppresses delegated ancestors on the second dispatch (jsdom, observed). Re-dispatching one Event object is unusual enough that I left it out.
- `.capture.stop` calls `stopPropagation()` at the mount root rather than at the declaring element, so a native capture listener on an element *between* the root and the declaring element is skipped where real DOM would run it first (observed).
- A literal `{:data-sema-mods-click "prevent stop"}` attribute forges modifiers past all validation (observed) — only reachable from the app's own SIP, so not a boundary I'd call broken.
- `dom/event-form-data` from an `:on-click` on a submit button omits that button's own name/value (click events carry no `.submitter`), while the same read from `:on-submit` includes it.
- In the SVG namespace `setAttribute` does not lowercase, so `:on-Click` writes `data-sema-on-Click`; jsdom's `hasAttribute` matches it anyway, which a real browser should not. Nobody writes `:on-Click`, so I dropped it — but it is a live example of jsdom being more lenient than Chrome.

### packages/sema-web (@sema-lang/sema-web) plus a workspace-wide sweep of crates/ for the debug_assert side-effec

ANGLE 1 (side effects inside debug_assert*) — DONE, essentially clean. Wrote a paren-matching Python scanner over all *.rs in crates/ that extracts each full debug_assert!/debug_assert_eq!/debug_assert_ne! body (multi-line and nested-paren aware, string-literal skipping) and greps the body for ~35 mutating method names (pop/take/replace/insert/remove/push/next/swap/drain/borrow_mut/fetch_*/set/write/send/advance/clear/truncate/entry/store/...). Of 57 debug_assert sites, exactly ONE real hit: crates/sema-wasm/src/driver.rs:155. The only other hit was the regression guard's own string literal in restricted.rs:1733 (false positive). I hand-inspected the plausible-looking survivors and they are all pure reads over prior let-bindings: timer.rs:47 (`removed`), fs_watch.rs:126 (`previous`), driver.rs:708 (`registered`), dap/server.rs:942 (`pending.len()`), eval.rs:1028 (`replaced`), cycle.rs:512 (`entry.get().strong_count()`), io.rs:2185-2186, stream.rs:2122.

ANGLE 2 (#[cfg(debug_assertions)] changing behaviour) — DONE, CLEAN, no finding. Only two such blocks exist in the whole workspace, both in crates/sema-core/src/value.rs:1211 and :1216. Both are diagnostics-only: the `let count = Rc::strong_count(&rc)` at 1212 is consumed solely by the assert, and the unconditional `Rc::into_raw` at 1213 sits OUTSIDE the cfg, so the boxing behaviour is identical in both profiles. I also grepped for `cfg!(debug_assertions)` and `cfg(not(debug_assertions))` — zero occurrences.

ANGLE 3 (dist bundle vs vitest source path) — DONE, this is where the real bugs were. Checked tsup define/replace (none used), minify (off, so no name-mangling reliance), import.meta/process.env (only src/testing.ts, correctly platform:"node" and kept out of the browser entry), and `sideEffects: false` — I verified that claim is HONEST by grepping every src/*.ts for top-level side-effect statements (customElements.define / document.* / window.* / bare top-level calls); there are none, so a consumer's tree-shaker cannot drop anything load-bearing. I verified the tsup `external: ["./index.js"]` guarantee actually holds in the built output (dist/testing.js is 7 KB and still contains the literal `await import("./index.js")`, so there is NO second copy of the runtime and no duplicate @preact/signals-core — that comment's worry is real but handled; @preact/signals-core is a `dependency` so tsup externalises it in both configs). I then drove the SHIPPED dist/index.js in real Chrome via a throwaway Playwright fixture reusing e2e/fixtures/scripts/counter.sema: it mounted, rendered "0", and reacted to a click with "1" — so the bundle itself is not broken, which is why I classified the dist coverage gap as coverage rather than a live break. I probed real Node/TypeScript resolution of the package through its own exports map from a temp project with a node_modules symlink — that is what surfaced findings 1 and 3.

BASELINE VERIFICATION — at session start I independently confirmed 846 tests / 30 files passing (exit 0), tsc exit 0, build exit 0. The e2e claim of "green at 67" did NOT hold: the suite is 84 tests and I measured 80 passed / 4 failed.

RETRACTED HYPOTHESIS (recorded because it cost real time and a reader should not redo it). I initially believed packages/sema-wasm/pkg/sema_wasm_bg.wasm was STALE relative to the restricted.rs fix and that this explained a `RuntimeError: unreachable` I saw. I forced a rebuild via `npm run build:wasm` and the trap disappeared, which seemed to confirm it. It does NOT: the cargo line in that build log reads "Finished `release` profile [optimized] in 0.33s", i.e. nothing recompiled, so the pre-existing .wasm already contained the fix and my rebuild only re-ran wasm-bindgen/wasm-opt. I also verified `npm run build:wasm` is a RELEASE build (wasm-pack defaults to release; the log confirms), so there is no accidental debug_assertions-on browser artifact either. Both of those worries are dead — do not chase them.

CRITICAL CAVEAT ON ALL E2E OBSERVATIONS: another agent was mutating this working tree throughout my session. Mid-run, e2e/tests/zzadv.spec.ts + its fixtures were DELETED and e2e/tests/zzq4res.spec.ts + tests/zzq4res-probe.test.ts APPEARED. That is what produced Playwright's "Test not found in the worker process. Make sure test title does not change." for zzprobe.spec.ts (a spec file rewritten between the list and run phases). It also means the unit suite now reports 13 failures, ALL of them in the other agent's in-flight tests/zzq4res-probe.test.ts — that is their WIP, not a finding, and I have excluded it. The zzadv-derived lifecycle failures I saw (a shared on-unmount handle across two instances erroring `Unknown callback handle: 1` and losing the second teardown; `component/unmount!` from inside an effect body leaving mountedComponentsById = [1,2,3] with the cleanup never run; zombie re-renders continuing after unmount) looked like genuine product bugs and their probe emitted real internal runtime errors rather than mere assertion mismatches — but I am NOT reporting them as findings because the probes were deleted under me and I cannot re-run them. Someone should re-derive them deliberately.

NOT FIXED ANYTHING (report-only phase). TREE LEFT AS FOUND: I created exactly three throwaway files (e2e/fixtures/tmpdist.html, e2e/fixtures/scripts/init-tmpdist.ts, e2e/tests/tmpdist.spec.ts) and deleted all three; `find` for *tmpdist* now returns nothing. I regenerated dist/ (npm run build) and packages/sema-wasm/pkg/ — both are fully gitignored and regenerable, and the other agent rebuilt dist/ after me anyway. test-results/ is gitignored Playwright output. No git add/commit/push.

---

## 2026-09-01 sweep — additional findings

A second pass over `packages/sema-web`, the `sema web` dev server, and the
web docs. Each item is fixed unless marked otherwise.

| # | Finding | Fix |
|---|---|---|
| 38 | A Sema error thrown inside a `ws/listen` handler (`:on-open` when deferred, `:on-message`, `:on-close`, `:on-error`) escaped into WebSocket event dispatch and never reached `onerror`; a throwing `:on-close` also skipped the registry cleanup, leaking the socket and its callbacks. | All four handlers run through one guard that routes to `ctx.onerror`; cleanup runs in `finally`. Tests in `tests/ws.test.ts`. |
| 39 | `ws/connect` inside a component was not owned by it: unmounting left the socket open. | The socket registers as an owned `websocket` stream (`SocketRegistration.streamId`), closed by the normal unmount drain. Test in `tests/ws.test.ts`. |
| 40 | `store/remove!`, `store/clear!`, `store/keys`, `store/has?` and the `session-*` variants threw when storage is blocked, contradicting `store.md`. | Every binding is guarded and returns a fallback. Test in `tests/store.test.ts`. |
| 41 | `http/event-source`'s completion promise was never `.catch`ed, so a throw from the SSE pump's `finally` became an unhandled rejection with no attribution. | Routed to `ctx.onerror` as `event-source:<id>`. |
| 42 | `llmProxy.timeout` was documented with a default but never read. | Documented as reserved; removed from the examples. |
| 43 | Dev server: an import that does not exist was traced in non-strict mode, so the page failed at runtime with "import nope: operation not supported on this platform". | `web_prepare_send` uses strict tracing and shows a build error naming the import. |
| 44 | Dev server: build-error text and the entry name were interpolated into the shell HTML unescaped. | `html-escape` in `dev_server.sema`. |
| 45 | Dev server: the LLM proxy handlers read fields off any JSON body, so a missing field surfaced as a type error from `llm/*`. | `with-body` validates the shape and required fields; `400 {"error": ...}`. |
| 46 | Dev server: `--host 0.0.0.0` exposed the unauthenticated proxy silently. | A startup warning names the risk and `--no-llm`. `is_loopback_host`/`format_host_port` moved to `sema_core::net` and shared with the workflow viewer; the auto-open URL brackets IPv6 hosts. |
| 47 | `http/serve`: a handler that raises produced the bounded 500 with the cause logged nowhere. | The prelude wrapper logs `http/serve: handler for <method> <path> raised: <message>` to stderr; the 500 body contract is unchanged. |
| 48 | `examples/web-demo/chat.sema` had drifted from the e2e fixture copy the tests run. | Synced; `tests/web-demo-fixtures.test.ts` fails on any future drift. |
| 49 | `examples/sema-web-app/app.sema` told users (in the rendered UI) to run `make sema-web-example`; there is no Makefile. | `jake wasm.sema-web-example`. |
| 50 | `examples/web/counter.sema` read `sema-counter` from storage but never wrote it, and would have applied `string->number` to a decoded number. | Persists on every change and restores the number directly. |
| 51 | Docs: the Todo and Streaming Chat samples in `examples.md`/`llm.md` used `string=?`, which does not exist. | `equal?`. Also fixed: `/public/app.vfs` path, `css` class-name format, the `reactive: false` override claim, `message` described as a map helper, `INVALID_REQUEST`/`TIMEOUT` statuses, missing `ProxyConfig` fields, `router/link` optional args, `dom/get-style` inline-only, two-arg `http/event-source`, `put!` return value, `mount!` symbol form, `defcomponent` props default. |
| 52 | `publish-npm.yml` re-pinned `@sema-lang/sema-wasm` for `@sema-lang/sema` but not `@sema-lang/sema` for `@sema-lang/sema-web`. | Same `node -e` re-pin step. |
| 53 | `scripts/check-web-runtime-fresh.sh` did not fingerprint `package-lock.json` or `jake/wasm.jake`, so a bump of the vendored `@preact/signals-core`/`morphdom` never invalidated the lock. | Both added to `INPUT_PATHSPECS`. |

Not changed, recorded:

- `{:routes "oops"}` in `router/init!` still registers the route `/routes` (residual of #28): keyword colons are stripped at the WASM boundary, so the string value cannot be told apart from a bare table whose pattern is spelled `routes`, and a test pins that reading.
- `sse.ts` calls its TS-level `onEvent`/`onError`/`onClose` callbacks unguarded; every in-tree caller only sets a signal, so a throw there is a runtime bug, not app code.
- `releaseHandlesForSubtree` is linear in the number of live handles per removed node; a large list teardown is quadratic. No report of it being felt.
- The `jake test.web-e2e` dev-server Playwright suite is still not part of any CI workflow (it needs a browser and the `sema` binary).

