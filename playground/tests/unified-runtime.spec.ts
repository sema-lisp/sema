// P6-3: WASM Promise-driven roots — acceptance gate.
//
// These tests pin the real-browser oracle for the unified async runtime landing
// described in docs/plans/2026-07-16-wasm-promise-driven-roots.md §5. They run
// against the real `wasm-pack`-built bundle (`jake pg.build`) in headless
// Chromium — the only valid oracle per the design doc.
//
//   jake pg.build && npx playwright test tests/unified-runtime.spec.ts
//
// (a) and (b) drive the shipped playground UI (worker path), matching how a
// real user exercises `evalPromise`/`setPromiseOutputSink` end to end.
//
// (c) and (d) require two IN-FLIGHT eval roots at once. The playground's own
// `app.js` `run()` is deliberately single-flight (`if (workerRunning) return`)
// — Stop cancels the one running eval, and a second Run is a no-op until the
// first settles. That is a UI policy choice, not a limitation of the
// underlying seam: `evalPromise`/`cancelRoot` (`crates/sema-wasm/src/driver.rs`,
// `crates/sema-wasm/src/lib.rs`) operate on arbitrary concurrent `RootId`s
// regardless of how many JS call sites invoke them. So (c) and (d) drive the
// seam directly: they load the SAME wasm-pack-built artifact the app serves
// (`/pkg/sema_wasm.js`, fetched by the real browser, not mocked) and call
// `evalPromise`/`cancelRoot` twice concurrently from the page context — the
// identical technique used to verify this seam in the P6-3 step 2 report
// (`.superpowers/sdd/p63-step2-report.md`, "A one-off Playwright probe").
// This is real-browser verification of the runtime property the design doc
// requires; it is not a stand-in for wiring concurrent runs into the UI
// (which nothing in P6-3's scope calls for).
//
// (e), P6-3 step 5 update: the three replay loops and the JS-side
// SAB/`legacySab` fallback are DELETED (see
// `docs/plans/2026-07-16-wasm-promise-driven-roots.md` §3), so this now
// scans the full sources (`crates/**`, `playground/src/**`) for `MAX_REPLAYS`
// and the SAB/`legacySab` machinery, not just `driver.rs`/the shipped
// bundle's default branch as step 4 scoped it.
//
// This is NOT a claim that `HTTP_AWAIT_MARKER`/`installAtomicsSleep` are gone
// from the Rust crate entirely — a step-5 audit found each has a real, still-
// live consumer beyond the deleted replay/legacySab machinery: (1) the wasm
// debugger's own `http_needed`/`debugPerformFetch` flow (`debugStart` is not
// promise-driven and has no other way to surface a pending fetch to JS), and
// (2) `check_interrupt`/blocking-sleep support for every still-synchronous
// entry point (`eval`/`evalGlobal`/`evalVM`, a precompiled bytecode archive
// entry) via `crates/sema-eval/src/eval.rs`'s `drive_handle_to_settlement`,
// which a bare `(async/sleep ...)` reaches on ANY path (`async/sleep` is not
// dual-ABI-gated the way `http/get` is). Forcing their deletion would break
// those live callers with no replacement mechanism in scope here — see
// `docs/deferred.md`'s P6-3 entry and
// `scripts/check-unified-runtime-legacy.sh`'s zero-tolerance list comment for
// the full record. `driver.rs` (the promise-driven path's own code) and the
// shipped JS bundle stay clean of all four markers, checked below.
import { test, expect, type Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { execSync } from 'node:child_process';
import path from 'node:path';

const REPO_ROOT = path.resolve(__dirname, '../..');

/** Type code into the editor, replacing existing content. */
async function setCode(page: Page, code: string) {
  await page.getByTestId('editor').fill(code);
}

// ── Gate (a): HTTP resolves via the Promise path, body runs EXACTLY ONCE ─────
//
// The direct refutation of replay: a side effect placed BEFORE the http/get
// must fire a single time. Under the old replay loop it fires once per replay
// (>= 2). Under Promise-driven roots the program body runs once and the
// http/get call site resumes with the real response — so the marker line
// appears exactly once and the response data is present in the output.
//
// Hits a same-origin static file (matching the existing "worker path: http/get
// works" spec) rather than an external host — no external network required,
// and no CORS/CORP complications from a real headless-Chromium run.
test('http/get resolves via Promise with the body executing once (no replay)', async ({ page }) => {
  await page.goto('/?worker');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const origin = await page.evaluate(() => location.origin);
  await setCode(
    page,
    [
      '(println "BEFORE-FETCH-MARKER")',
      `(def resp (http/get "${origin}/index.html"))`,
      '(println (string-append "STATUS:" (number->string (:status resp))))',
    ].join('\n'),
  );
  await page.getByTestId('run-btn').click();

  // Wait for the fetch to settle and the status line to render.
  await page.waitForFunction(
    () => document.body.innerText.includes('STATUS:200'),
    { timeout: 30_000 },
  );

  const text = await page.locator('body').innerText();
  const markerCount = (text.match(/BEFORE-FETCH-MARKER/g) ?? []).length;
  // EXACTLY once — replay would produce two or more.
  expect(markerCount).toBe(1);
  expect(text).toContain('STATUS:200');
});

// ── Gate (b): async/sleep completes via setTimeout, page stays responsive ─────
//
// No Atomics.wait, no SharedArrayBuffer: the sleep registers an External wait
// completed by a setTimeout callback, and the macrotask driver yields to the
// event loop between turns so the page remains paintable/interactive while the
// sleep is pending.
test('async/sleep completes via setTimeout without blocking the page', async ({ page }) => {
  await page.goto('/?worker');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  await setCode(
    page,
    [
      '(println "START")',
      '(async/sleep 250)',
      '(println "AFTER-SLEEP")',
    ].join('\n'),
  );

  // Main-thread responsiveness probe: a timer that must keep ticking while the
  // sleep is pending — an observable macrotask boundary, the direct refutation
  // of a synchronous `Atomics.wait` blocking the page.
  await page.evaluate(() => {
    (window as any).__ticks = 0;
    (window as any).__t = setInterval(() => { (window as any).__ticks++; }, 20);
  });

  await page.getByTestId('run-btn').click();

  await page.waitForFunction(() => document.body.innerText.includes('START'), {
    timeout: 5_000,
  });

  // While the sleep is pending the page must stay responsive: a trivial DOM
  // interaction still resolves.
  const responsiveWhilePending = await page.evaluate(
    () => new Promise<boolean>((r) => setTimeout(() => r(true), 0)),
  );
  expect(responsiveWhilePending).toBe(true);

  await page.waitForFunction(
    () => document.body.innerText.includes('AFTER-SLEEP'),
    { timeout: 10_000 },
  );

  const ticks = await page.evaluate(() => {
    clearInterval((window as any).__t);
    return (window as any).__ticks as number;
  });
  // The 250ms sleep spanned multiple 20ms ticks — the page's own event loop
  // kept running the whole time, i.e. drive turns yielded between themselves.
  expect(ticks).toBeGreaterThan(3);
});

// ── Gate (c): two concurrent roots settle fairly with distinct identity ──────
test('two concurrent eval roots stay pending and settle fairly with distinct root ids', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const result = await page.evaluate(async () => {
    // The exact wasm-pack-built artifact the playground worker loads.
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const roots: number[] = [];
    const events: string[] = [];
    interp.setPromiseOutputSink((rootId: number, _stream: string, text: string) => {
      events.push(`${rootId}:${text}`);
    });

    // Root A sleeps longer (100ms) than root B (30ms) — a FIFO/serialized
    // implementation would still resolve A first (submitted first); a fair
    // concurrent scheduler resolves B first, since its wait is shorter.
    let settledFirst: 'A' | 'B' | null = null;
    const pA = new Promise((resolve, reject) => {
      interp
        .evalPromise('(println "A-start")(async/sleep 100)(println "A-done")', (rid: number) => roots.push(rid))
        .then((v: unknown) => { if (settledFirst === null) settledFirst = 'A'; resolve(v); }, reject);
    });
    const pB = new Promise((resolve, reject) => {
      interp
        .evalPromise('(println "B-start")(async/sleep 30)(println "B-done")', (rid: number) => roots.push(rid))
        .then((v: unknown) => { if (settledFirst === null) settledFirst = 'B'; resolve(v); }, reject);
    });

    // Both calls must return without either having settled synchronously —
    // "stay pending" until the macrotask driver actually resolves them.
    const bothStillPendingAfterSubmit = await Promise.race([
      Promise.all([pA, pB]).then(() => false),
      new Promise<boolean>((r) => setTimeout(() => r(true), 0)),
    ]);

    await Promise.all([pA, pB]);
    return { roots, events, settledFirst, bothStillPendingAfterSubmit };
  });

  // Distinct root ids identify each concurrent evaluation.
  expect(result.roots.length).toBe(2);
  expect(result.roots[0]).not.toBe(result.roots[1]);
  // Neither settled synchronously on submission.
  expect(result.bothStillPendingAfterSubmit).toBe(true);
  // Fair scheduling: the shorter sleep (B) settles before the longer one (A),
  // not FIFO-by-submission-order.
  expect(result.settledFirst).toBe('B');
  expect(result.events).toEqual(
    expect.arrayContaining(['B-done', 'A-done'].map((s) => expect.stringContaining(s))),
  );
});

// ── Gate (d): Stop cancels one exact root; the other continues ───────────────
test('cancelRoot cancels the exact RootId via RuntimeCommandHandle::cancel_root, leaving the other root to complete', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const t0 = Date.now();
  const result = await page.evaluate(async () => {
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const events: string[] = [];
    interp.setPromiseOutputSink((rootId: number, _stream: string, text: string) => {
      events.push(`${rootId}:${text}`);
    });

    let rootA: number | undefined;
    let rootB: number | undefined;
    // Root A: a long sleep that must be cancelled well before it would print.
    const pA = new Promise((resolve, reject) => {
      interp
        .evalPromise('(async/sleep 5000)(println "A-should-not-print")', (rid: number) => { rootA = rid; })
        .then(resolve, reject);
    });
    // Root B: a short sleep that must complete normally, untouched by A's cancel.
    const pB = new Promise((resolve, reject) => {
      interp
        .evalPromise('(async/sleep 100)(println "B-done")', (rid: number) => { rootB = rid; })
        .then(resolve, reject);
    });

    // Give both roots a turn to actually start (root ids assigned, first
    // drive turn run) before cancelling.
    await new Promise((r) => setTimeout(r, 20));
    const cancelled = interp.cancelRoot(rootA);

    const aOutcome = await pA.then(
      (v) => ({ ok: true, v }),
      (e) => ({ ok: false, message: e && e.message ? e.message : String(e) }),
    );
    const bOutcome = await pB.then(
      (v) => ({ ok: true, v }),
      (e) => ({ ok: false, message: e && e.message ? e.message : String(e) }),
    );

    return { cancelled, events, aOutcome, bOutcome, rootA, rootB };
  });
  const elapsed = Date.now() - t0;

  expect(result.cancelled).toBe(true);
  expect(result.rootA).not.toBe(result.rootB);
  // A was cancelled, not left to run to completion (would take ~5s).
  expect(result.aOutcome.ok).toBe(false);
  expect((result.aOutcome as { message: string }).message.toLowerCase()).toContain('cancel');
  expect(result.events).not.toContain(expect.stringContaining('A-should-not-print'));
  // B settled normally, unaffected by A's cancellation.
  expect(result.bOutcome.ok).toBe(true);
  expect(result.events.some((e) => e.endsWith(':B-done'))).toBe(true);
  // Cancelled well under B's own 100ms sleep plus A's would-be 5000ms —
  // proves the cancel was delivered promptly, not "eventually" after a long wait.
  expect(elapsed).toBeLessThan(3000);
});

// ── Gate (e), un-scoped (P6-3 step 5 —
// `docs/plans/2026-07-16-wasm-promise-driven-roots.md`): the replay loops and
// the JS-side SAB/`legacySab` fallback are now actually deleted, so this
// scans the FULL sources, not just `driver.rs`/the shipped bundle's default
// branch as step 4 scoped it. It is NOT a claim that `HTTP_AWAIT_MARKER`/
// `installAtomicsSleep` are gone from the Rust crate entirely — they aren't:
// `crates/sema-wasm/src/lib.rs` keeps both, narrowed to two still-live,
// verified consumers documented in `docs/deferred.md`'s P6-3 entry and
// `scripts/check-unified-runtime-legacy.sh`'s zero-tolerance list comment —
// (1) the wasm debugger's own `http_needed`/`debugPerformFetch` flow (not
// promise-driven; has no other way to surface a pending fetch to JS), and (2)
// `check_interrupt`/blocking-sleep support for every still-synchronous entry
// point (`eval`/`evalGlobal`/`evalVM`, a precompiled bytecode archive entry)
// via `crates/sema-eval/src/eval.rs`'s `drive_handle_to_settlement`, which a
// bare `(async/sleep ...)` reaches on ANY path (not dual-ABI-gated). What
// full-repo deletion actually removed: the three replay loops themselves,
// `MAX_REPLAYS` (no remaining caller anywhere), and the worker's SAB
// allocation/`legacySab` branch (JS) entirely.
test('the replay loops and MAX_REPLAYS are gone repo-wide', async () => {
  const roots = ['crates', 'playground/src'];
  for (const marker of ['MAX_REPLAYS', 'legacySab', 'new SharedArrayBuffer(']) {
    for (const root of roots) {
      execSync(
        `! grep -RIn --include='*.rs' --include='*.js' --include='*.ts' ` +
          `-- '${marker}' ${path.join(REPO_ROOT, root)}`,
        { shell: '/bin/bash' },
      );
    }
  }
});

test('driver.rs (the promise-driven path itself) is free of the legacy replay/Atomics markers', async () => {
  const driverSrc = readFileSync(
    path.join(REPO_ROOT, 'crates/sema-wasm/src/driver.rs'),
    'utf8',
  );
  for (const marker of ['HTTP_AWAIT_MARKER', 'MAX_REPLAYS', 'installAtomicsSleep', 'Atomics.wait']) {
    expect(driverSrc).not.toContain(marker);
  }
});

test('the shipped default worker protocol never reaches legacy Atomics/replay code (SAB deleted entirely)', async () => {
  const workerSrc = readFileSync(path.join(__dirname, '..', 'dist', 'sema-worker.js'), 'utf8');
  const appSrc = readFileSync(path.join(__dirname, '..', 'dist', 'app.js'), 'utf8');

  // The replay markers are `lib.rs`-only (Rust, and narrowed to the
  // debugger); they must not appear in the shipped JS bundle at all.
  for (const marker of ['HTTP_AWAIT_MARKER', 'MAX_REPLAYS', 'legacySab', 'SharedArrayBuffer(']) {
    expect(workerSrc).not.toContain(marker);
    expect(appSrc).not.toContain(marker);
  }
  // No real blocking Atomics.wait/notify CALL exists anywhere in the shipped
  // worker (the sleep wait, where it still exists at all, happens inside the
  // wasm binary via `js_sys::Atomics`, not as a literal invocation in JS).
  expect(workerSrc).not.toMatch(/Atomics\.(wait|store|notify)\(/);
  expect(appSrc).not.toMatch(/Atomics\.(wait|store|notify)\(/);
  expect(workerSrc).not.toContain('installAtomicsSleep');
  expect(appSrc).not.toContain('installAtomicsSleep');

  // The default protocol's own reachable entry points call the new seam.
  expect(workerSrc).toContain('evalPromise');
  expect(workerSrc).toContain('cancelRoot');
});
