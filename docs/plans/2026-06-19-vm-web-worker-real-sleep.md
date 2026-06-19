# VM on a Web Worker — real `async/sleep` in the playground

**Status:** proposed / feasibility-complete (not started)
**Date:** 2026-06-19
**Goal:** Move the WASM VM eval off the browser main thread onto a dedicated Web Worker so the worker can block on `Atomics.wait` and give **real wall-clock `async/sleep`** in the playground. Keep the deterministic virtual clock (shipped in 1.18.0) for ordering; layer real waiting on top. Today WASM advances the virtual clock instantly because the UI thread must not block.

This document is the output of a 6-dimension feasibility study (coupling, scheduler integration, COOP/COEP, build mechanics, message protocol, prior art) + synthesis. File:line references were verified against the tree at this date.

---

## 1. Verdict

**GO-WITH-CAVEATS.** The sleep primitive is a clean ~10-line drop-in at `crates/sema-vm/src/scheduler.rs:552-559`, and cross-origin isolation is a ~2-line `vercel.json` change with verified-safe subresources. The biggest gating risk is **not** the sleep primitive but the **worker re-plumbing of HTTP and the ~30 synchronous wasm-bindgen methods**: today's async-HTTP design is a main-thread `throw-marker → fetch → replay-whole-program` loop that is incompatible with a worker that blocks on `Atomics.wait`, and currently-synchronous VFS/debug calls must become async RPC.

## 2. Recommended architecture

**Single-threaded dedicated module Worker + a small (4-byte) control `SharedArrayBuffer` for `Atomics.wait`. NOT full wasm shared-memory threads.**

- The goal needs only *blocking sleep*, not parallelism. `Atomics.wait(Int32Array, 0, 0, delta_ms)` works on any standalone SAB-backed `Int32Array`; it does **not** require `shared` wasm linear memory, `+atomics,+bulk-memory` RUSTFLAGS, or a nightly `build-std`. Prior art: Pyodide/comsync, CoWasm.
- This keeps the wasm pkg **byte-identical to today's `wasm-pack build --target web`** (`Makefile:179`). A threaded rebuild is a separate, larger effort with no payoff for this goal.
- **All Sema runtime state already lives in wasm thread-locals** (`crates/sema-wasm/src/lib.rs:10-39`: `OUTPUT`, `LINE_BUF`, `VFS`, `VFS_DIRS`, `HTTP_CACHE`, `DEBUG_SESSION`; scheduler at `crates/sema-vm/src/scheduler.rs:259`). Moving the wasm instance onto the worker moves *all* of it automatically — no state-splitting.
- **Virtual clock stays; real wait layers on top.** The all-blocked branch computes `delta = target_time - virtual_now` then unconditionally sets `virtual_now = target_time` (line 559). The worker arm keeps line 559 (determinism oracle) and inserts `Atomics.wait(delta)` before it — structurally identical to how native pairs `std::thread::sleep(delta)` with the same jump. `delta` is bounded ≤ 1 day by the `MAX_SLEEP_MS`/`MAX_TIMEOUT` caps, so the worker can't wedge.

## 3. What moves to the worker vs stays main-thread

| Concern | Disposition |
|---|---|
| **Eval** (VM + scheduler) | **Worker.** `init_scheduler`'s thread-local must live on the worker; the synchronous `eval_str_compiled` path can now block. |
| **Sleep pacing** (`scheduler.rs:552-559`) | **Worker.** `Atomics.wait` on control SAB. Must no-op / instant-advance when no SAB present (`Atomics.wait` throws on the main thread) — that's the fallback path. |
| **VFS state** (`lib.rs:17-28`) | **Worker.** UI tree/preview become async RPC. |
| **VFS persistence** (localStorage/session/IndexedDB, `vfs-backends.js`) | **Main thread** (localStorage is main-thread-only). Bridge via **batched** `dumpVFS`/`loadVFS` snapshot messages — per-file chatter is the perf trap. |
| **HTTP** | **Worker `fetch`** (recommended). Deletes the entire `MAX_REPLAYS` replay-with-marker hack. Caveat: `web_sys::window()` (`lib.rs:320`) doesn't exist in a worker — use `WorkerGlobalScope`/`self`. **Never block on `Atomics.wait` for HTTP** — reserve Atomics strictly for sleep. |
| **Output** (`println`) | **Worker, streamed** (`{type:'output', runId, line}`). Enables live output + cancellation; needs coalescing (flush per N lines / frame) to avoid flooding postMessage. |
| **Cancellation** | **Soft cancel:** an `Atomics.store` flag the VM polls at the existing step-count poll point, also used as the `Atomics.wait` wake condition (interruptible sleep). Avoid hard `worker.terminate()` (wipes Rc/VFS/defines, forces ~3MB respawn). |
| **`registerFunction`** (Sema→JS sync callback) | **Worker** if the callback can move there; an Atomics-blocked worker cannot synchronously call a main-thread callback. **Audit examples first.** |

## 4. The cross-origin-isolation (COOP/COEP) gate

**Required:** `Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy: require-corp`, or `self.crossOriginIsolated` is false and SAB/`Atomics.wait` are unavailable.

- **Vercel change (low-risk):** append one `source: "/(.*)"` rule to the existing `headers` array in `playground/vercel.json` (today it has only the `/pkg/*.wasm` Content-Type rule). Deploys as the standalone `sema-playground` project — does not affect `sema-lang.com`.
- **What could break (verified safe):** the only cross-origin subresource is Google Fonts (`index.html:23-25`); both `fonts.googleapis.com` and `fonts.gstatic.com` return `CORP: cross-origin` + `ACAO: *`, so they load under `require-corp`. Prefer `require-corp` over `credentialless` (fonts already send CORP; credentialless lacks older-Safari support).
- **Real exposure:** user `http/*` examples — third-party API responses lacking CORP/CORS fail under `require-corp`. Mitigation: a same-origin fetch proxy, or document the limitation.
- **Fallback when COI unavailable** (older Safari, embeds, header-stripping SWs): feature-detect `self.crossOriginIsolated`/`SharedArrayBuffer`; if absent, run today's main-thread instant-virtual-clock path unchanged. Ship the worker path as **progressive enhancement.**
- **Dev/E2E parity:** `vercel.json` headers don't apply to `npx serve` or Playwright — needs a `serve.json` (or a static server) emitting COOP/COEP, else SAB silently disables in tests.

## 5. Phased plan (risk retired early)

- **M1 — COI + Atomics.wait spike (no Sema changes). ✅ DONE 2026-06-19.** Self-contained spike in `playground/spike/` (`index.html` + `worker.js` + `serve.json` COOP/COEP + `spike-check.mjs` headless-Chromium check). *Accept — PASSED:* headers verified via curl; in headless Chromium `crossOriginIsolated===true`, SAB available, worker **blocked 1005ms** on `Atomics.wait` (`"timed-out"`), main thread logged **20 ticks during the block** (responsive). Retires the headline COI+Atomics risk. Run: `cd playground && npx serve spike -l 8911 & node spike/spike-check.mjs`. Left for the shipping milestone: port the COOP/COEP headers to the real `playground/vercel.json` + dev `serve.json` and confirm Google Fonts load under `require-corp` on deploy.
- **M2 — Scheduler sleep hook (Rust). ✅ DONE 2026-06-19.** Added `sema_core::blocking_sleep_ms(delta)` + `set_blocking_sleep_callback`/`clear_blocking_sleep_callback` (a `fn(u64)` callback, mirroring the spawn/run_scheduler/cancel pattern in `async_signal.rs`). The scheduler's clock-advance now routes through it (the `#[cfg]` fork is removed from `scheduler.rs`); default = `std::thread::sleep` on native, instant no-op in wasm; a host can override. *Accept — PASSED:* native + wasm32 build clean; native test `blocking_sleep_hook_receives_clock_advances` proves the hook fires with deltas summing to total virtual time while ordering stays a,b,c; full suite + lint green. **Deferred to M3 (needs the worker/SAB to be testable):** the `#[wasm_bindgen]` `atomics_sleep` extern + a `SemaInterpreter.installAtomicsSleep(sab)` that stores the SAB in a thread-local and registers a `fn(u64)` reading it (fn-pointers can't capture). `Atomics.wait` throws on the main thread, so this is only exercisable once the worker exists.
- **M3 — Worker bootstrap + eval RPC. ✅ DONE 2026-06-19.** Landed the deferred M2 wasm bit (`#[wasm_bindgen] installAtomicsSleep(Int32Array)` + `SLEEP_I32` thread-local + `worker_atomics_sleep` registering the sema-core hook); `sema-worker.js` (module worker: loads pkg, installs SAB sleep hook, runs `evalVMAsync`, posts `{value,output,error}`); `worker-client.js` (RPC + `?worker` gate requiring cross-origin isolation); `build.mjs` 2nd entry → `dist/sema-worker.js`; `app.js` `run()` routes through the worker when active (+ live "Running…"/"Error" status); `serve.json` COOP/COEP for dev/e2e. **Gated behind `?worker`** so the default playground + all existing e2e are unchanged. Production `vercel.json` NOT changed yet (prod stays main-thread/HTTP-working until M5). *Accept — PASSED:* new e2e proves a 100/200/300ms concurrent-sleep program takes >280ms real wall-clock on the worker, main thread stays responsive (ticks advance), output `[a,b,c,done]`. Full e2e 54/54; clippy + fmt clean.
- **M4 — VFS over the worker (snapshot mirror). ✅ DONE 2026-06-19.** Chose the *mirror* design over async-ifying the UI: added wasm `dumpVfs()` → `{files,dirs}` JS object + `loadVfs(snapshot)`. The worker carries the VFS in/out of the eval round-trip (`{type:'eval', …, vfs}` → `{type:'result', …, vfs}`); `app.js` seeds the worker from the main-thread interp before each run and `loadVfs`es the result back into it after, keeping the main-thread interp a **synchronous mirror** so all existing tree/preview/persistence/upload code is unchanged — no `buildVfsTree` async rewrite, no per-file postMessage chatter (one snapshot per run). *Accept — PASSED:* new e2e writes `/from-worker.txt` during a worker run and asserts it appears in the (mirror-backed) file tree. Full e2e 55/55; clippy + fmt clean.
- **M5 — HTTP on the worker. ✅ (core) DONE 2026-06-19.** Chose **synchronous XHR** over async fetch+replay: on the worker (`web_sys::window().is_none()`), `wasm_http_request` does a blocking `XMLHttpRequest` (`perform_fetch_sync`) that returns the `{:status :headers :body}` directly — no `HTTP_AWAIT_MARKER`/replay, and it composes with real `Atomics.wait` sleeps (no whole-program re-runs, no re-slept time). Main-thread path keeps the fetch+replay (unchanged, dual-context). Added the `XmlHttpRequest` web-sys feature. *Accept — PASSED:* new e2e does `(:status (http/get "<origin>/index.html"))` on the worker → 200, no `window` error. Full e2e 57/57; clippy + fmt clean.
  - **Sub-decisions (2026-06-19):**
    - **No regression from COEP:** CORS-mode requests with `Access-Control-Allow-Origin` satisfy `require-corp`, so every endpoint that works today (it already passes CORS) keeps working; only non-CORS endpoints (which fail today anyway) stay blocked.
    - **M5b proxy — DEFERRED (documented):** a same-origin proxy would *add* non-CORS support but an open proxy on sema.run is an SSRF/abuse vector. Not building it now; non-CORS cross-origin targets surface a clear error. Revisit with a guarded (allowlisted) proxy if needed.
    - **M5c prod COI — STAGED, not committed:** enabling `require-corp` in `playground/vercel.json` is safe (above) and is what turns on `sema.run/?worker` real-sleep, but it's a live-behavior change on deploy, so it's intentionally **not** added to `vercel.json` yet to avoid a stray deploy flipping it. Enable it deliberately alongside the M7 default-flip (or when explicitly chosen). Per-request timeout is not applied on the sync-XHR path (worker can be cancelled via M6 instead).
- **M6 — Soft cancellation + cancellable sleep.** Cancel flag at the step poll point (`lib.rs:1578`) + as the `Atomics.wait` wake condition; Stop button. *Accept:* running/mid-sleep program stops promptly; worker survives (state preserved).
- **M7 — Debugger over the worker + fallback hardening.** Port `debugStart/Continue/Step/Poll` to message RPC; re-express the `setTimeout(0)` yield loop (`app.js:701`); finalize the `!crossOriginIsolated` legacy fallback. *Accept:* breakpoints/stepping work over the worker; non-isolated browsers run legacy mode cleanly.

## 6. Effort & risk

| Milestone | Effort | Risk | Dominant risk |
|---|---|---|---|
| M1 COI + Atomics spike | S | Low | header propagation; dev-server parity |
| M2 Scheduler sleep arm | S | Low | main-thread `Atomics.wait` throws — fallback must no-op |
| M3 Worker bootstrap + eval RPC | L | Med | JSON output contract; streaming backpressure |
| M4 VFS over worker | M | Med | async-ifying recursive sync VFS calls without chatter |
| M5 HTTP on worker fetch | M | High | `window()`→worker fetch; COEP breaks non-CORP targets; sleep+HTTP coordination |
| M6 Soft cancellation | S | Med | poll-point cost; interruptible-wait protocol |
| M7 Debugger + fallback | M | Med | stateful poll-based debugger → async RPC; dual-mode upkeep |

Net ~**L**: one S spike retires the headline feasibility risk; the bulk is worker RPC re-plumbing in M3/M5, not the Atomics primitive.

## 6b. Performance, embedders, and compatibility

**Scope decision (2026-06-19): target hosted sema.run too** — accept the COEP exposure on `http/*` examples, mitigated by a same-origin fetch proxy (folded into M5).

**Performance.** Not a raw-eval-speed play: the VM runs at the same speed on a worker, and it stays single-threaded (`Rc` everywhere — a worker does *not* unlock parallel Sema tasks; that would be a separate rewrite). The wins are: (1) **UI responsiveness** — long programs no longer freeze the tab; (2) **streamed `println`** instead of end-of-run batch dump (enables progress UI); (3) **cancellation**; and (4) a **real throughput win for HTTP-heavy programs** — today `evalVMAsync` re-runs the whole program from the top per request (replay-with-marker, ≤50×, ~O(N²) in requests); worker `fetch` with real suspend/resume runs it once (O(N)) and deletes the hack.

**JS embedders.** No forced change. `@sema-lang/sema-wasm` stays main-thread-loadable with an unchanged API. M2's sleep-arm is **gated on a SAB-backed callback** (installed only by the playground worker); a plain main-thread embedder has no SAB → `async/sleep` keeps advancing the virtual clock instantly → identical to today. Embedders may *optionally* adopt the worker pattern for real sleep. **Hard constraint:** M5's HTTP refactor (`web_sys::window()` → worker `self`) must stay **dual-context** so main-thread embedders' HTTP keeps working.

**Compatibility / does it break wasm consumers?** Main-thread consumers: nothing breaks if (a) the sleep-arm no-ops without a SAB, (b) HTTP stays dual-context, and (c) the **non-isolated fallback is solid** (older Safari / embeds / FF private mode must run the legacy main-thread instant-clock path — mandatory, M7). The one *intentional* regression from the hosted-too decision: `http/*` examples to non-CORP/CORS endpoints fail under `require-corp` — mitigated by the same-origin proxy. Also verify nothing relied on the deleted HTTP replay-cache request-coalescing.

## 7. Open questions (need a decision)

1. **HTTP model:** confirm HTTP uses worker `fetch` (Atomics = sleep-only). If synchronous `http/get` (`lib.rs:924`) must keep working, you need the harder Atomics-block-then-main-thread-fetch path. *Recommend: async worker fetch.*
2. **Top-level `sleep` no-op (`lib.rs:697-702`)** in scope for real pacing, or only scheduler `async/sleep`?
3. **Always-on real pacing vs flag-gated?** (CI/tests may want instant-advance even on the worker.)
4. **Cancel = soft flag (preserves defines/VFS) vs hard terminate (wipes state).** *Recommend soft.*
5. **Does the debugger run on the worker, and keep the instant clock during interactive stepping?** (real sleep while stepping is semantically odd.)
6. **`registerFunction` audit** — does any shipped example call a main-thread JS callback during eval?
7. **COEP `require-corp` vs `credentialless`** given third-party `http/*` targets — accept breakage, add a proxy, or document?
8. **Is real sleep wanted on hosted sema.run, or local/CLI-parity only?** If the COEP cost on http examples is unacceptable, real sleep could be local-only while the deploy keeps the instant clock.
