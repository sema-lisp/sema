// P6-3: WASM Promise-driven roots — acceptance gate.
//
// These tests pin the real-browser oracle for the unified async runtime landing
// described in docs/plans/archive/2026-07-16-wasm-promise-driven-roots.md §5. They run
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
// `docs/plans/archive/2026-07-16-wasm-promise-driven-roots.md` §3), so this now
// scans the full sources (`crates/**`, `playground/src/**`) for `MAX_REPLAYS`
// and the SAB/`legacySab` machinery, not just `driver.rs`/the shipped
// bundle's default branch as step 4 scoped it.
//
// Promise-driven HTTP, timers, and debugging now own every admissible browser
// suspension. Synchronous WASM entry points reject suspension instead of
// retaining a replay or blocking-host bridge. The source and artifact checks
// below therefore reject those retired markers across both Rust and shipped JS.
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
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
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
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
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
  expect(result.events.some((e: string) => e.includes('A-should-not-print'))).toBe(false);
  // B settled normally, unaffected by A's cancellation.
  expect(result.bOutcome.ok).toBe(true);
  expect(result.events.some((e) => e.endsWith(':B-done'))).toBe(true);
  // Cancelled well under B's own 100ms sleep plus A's would-be 5000ms —
  // proves the cancel was delivered promptly, not "eventually" after a long wait.
  expect(elapsed).toBeLessThan(3000);
});

// ── Gate (e): each interpreter owns its Promise driver ─────────────────────
test('separate interpreters settle concurrent suspending evalPromise and evalAsync roots', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
      const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();

    let rootA: number | undefined;
    const a = interpA
      .evalPromise('(async/sleep 80) "A-ok"', (rid: number) => { rootA = rid; })
      .then(() => 'fulfilled', () => 'rejected');
    const b = interpB
      .evalAsync('(async/sleep 30) "B-ok"')
      .then(() => 'fulfilled', () => 'rejected');

    const settled = await Promise.race([
      Promise.all([a, b]).then((outcomes) => ({ timedOut: false, outcomes })),
      new Promise<{ timedOut: true; outcomes: string[] }>((resolve) => {
        setTimeout(() => resolve({ timedOut: true, outcomes: [] }), 1_500);
      }),
    ]);

    // Settling the last root unregisters the driver's weak wake route. A
    // later external operation must re-register it and still receive the
    // executor completion for this exact interpreter.
    const followUp = settled.timedOut
      ? 'skipped'
      : await Promise.race([
          interpA
            .evalPromise(`(:status (http/get "${location.origin}/index.html"))`, undefined)
            .then(() => 'fulfilled', () => 'rejected'),
          new Promise<string>((resolve) => setTimeout(() => resolve('timed-out'), 1_500)),
        ]);

    return { rootA, followUp, ...settled };
  });

  expect(result.timedOut).toBe(false);
  expect(result.rootA).toBeDefined();
  expect(result.outcomes).toEqual(['fulfilled', 'fulfilled']);
  expect(result.followUp).toBe('fulfilled');
});

test('cancelling one interpreter root cannot cancel a colliding root owned by another interpreter', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
      const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();

    let rootA: number | undefined;
    let rootB: number | undefined;
    const a = interpA
      .evalPromise('(async/sleep 5000) "A-should-not-complete"', (rid: number) => { rootA = rid; })
      .then(() => 'fulfilled', () => 'cancelled');
    const b = interpB
      .evalPromise('(async/sleep 100) "B-ok"', (rid: number) => { rootB = rid; })
      .then(() => 'fulfilled', () => 'rejected');

    await new Promise((resolve) => setTimeout(resolve, 20));
    const cancelled = interpA.cancelRoot(rootA);
    const settled = await Promise.race([
      Promise.all([a, b]).then((outcomes) => ({ timedOut: false, outcomes })),
      new Promise<{ timedOut: true; outcomes: string[] }>((resolve) => {
        setTimeout(() => resolve({ timedOut: true, outcomes: [] }), 1_500);
      }),
    ]);

    return { cancelled, rootA, rootB, ...settled };
  });

  expect(result.rootA).toBeDefined();
  expect(result.rootA).toBe(result.rootB);
  expect(result.cancelled).toBe(true);
  expect(result.timedOut).toBe(false);
  expect(result.outcomes).toEqual(['cancelled', 'fulfilled']);
});

test('separate interpreters isolate colliding-root Promise and compatibility output', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();

    const eventsA: string[] = [];
    const eventsB: string[] = [];
    let rootA: number | undefined;
    let rootB: number | undefined;
    interpA.setPromiseOutputSink((_root: number, _stream: string, text: string) => {
      eventsA.push(text);
    });
    interpB.setPromiseOutputSink((_root: number, _stream: string, text: string) => {
      eventsB.push(text);
    });

    await Promise.all([
      interpA.evalPromise(
        '(async/sleep 10)(println "A-promise-only")',
        (root: number) => { rootA = root; },
      ),
      interpB.evalPromise(
        '(async/sleep 40)(println "B-promise-only")',
        (root: number) => { rootB = root; },
      ),
    ]);

    const [compatA, compatB] = await Promise.all([
      interpA.evalAsync('(async/sleep 10)(println "A-compat-only")'),
      interpB.evalAsync('(async/sleep 40)(println "B-compat-only")'),
    ]);
    const promiseMarkers = (events: string[]) =>
      events.filter((text) => text.includes('promise-only'));
    const compatMarkers = (output: string[]) =>
      output.filter((text) => text.includes('compat-only'));

    return {
      rootA,
      rootB,
      eventsA: promiseMarkers(eventsA),
      eventsB: promiseMarkers(eventsB),
      compatA: compatMarkers(compatA.output),
      compatB: compatMarkers(compatB.output),
    };
  });

  expect(result.rootA).toBeDefined();
  expect(result.rootA).toBe(result.rootB);
  expect(result.eventsA).toEqual(['A-promise-only']);
  expect(result.eventsB).toEqual(['B-promise-only']);
  expect(result.compatA).toEqual(['A-compat-only']);
  expect(result.compatB).toEqual(['B-compat-only']);
});

test('a Promise output callback can clear itself reentrantly', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    const events: string[] = [];
    let clearError: string | null = null;

    interp.setPromiseOutputSink((_root: number, _stream: string, text: string) => {
      events.push(text);
      try {
        interp.setPromiseOutputSink(undefined);
      } catch (error) {
        clearError = error instanceof Error ? error.message : String(error);
      }
    });
    await interp.evalPromise('(println "first-only")', undefined);
    await interp.evalPromise('(println "second-must-not-be-observed")', undefined);

    return {
      clearError,
      markers: events.filter(
        (text) => text.includes('first-only') || text.includes('second-must-not-be-observed'),
      ),
    };
  });

  expect(result.clearError).toBeNull();
  expect(result.markers).toEqual(['first-only']);
});

test('synchronous evalVM points HTTP callers to evalPromise without leaking its marker', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    return interp.evalVM(`(http/get "${location.origin}/index.html")`);
  });

  expect(result.error).toContain(
    'http/get: synchronous WebAssembly evaluation cannot perform HTTP requests; use evalPromise',
  );
  expect(result.error).not.toContain('__SEMA_WASM_HTTP__');
});

test('a synchronous re-entry cannot drive or capture a pending Promise HTTP root', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const requestUrl = `${await page.evaluate(() => location.origin)}/index.html?promise-root-reentry=1`;
  let requestCount = 0;
  page.on('request', (request) => {
    if (request.url() === requestUrl) requestCount += 1;
  });

  const result = await page.evaluate(async (url) => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    const promiseEvents: Array<{ root: number; stream: string; text: string }> = [];
    interp.setPromiseOutputSink((root: number, stream: string, text: string) => {
      promiseEvents.push({ root, stream, text });
    });

    // Submission queues a macrotask. Re-enter synchronously before that task
    // can fire: the synchronous drive must select only its own fresh root.
    const pending = interp.evalPromise(
      `(println "promise-before")\n(def response (http/get "${url}"))\n(println "promise-after")\n(:status response)`,
      undefined,
    );
    const sync = interp.evalVM('(println "sync-only")\n(+ 1 2)');
    let promised: string | null = null;
    let promiseError: string | null = null;
    try {
      promised = await pending;
    } catch (error) {
      promiseError = error instanceof Error ? error.message : String(error);
    }
    return { sync, promised, promiseError, promiseEvents };
  }, requestUrl);

  expect(requestCount).toBe(1);
  expect(result.sync).toMatchObject({ value: '3', output: ['sync-only'], error: null });
  expect(result.promised).toBe('200');
  expect(result.promiseError).toBeNull();
  expect(result.promiseEvents.map(({ stream, text }) => [stream, text])).toEqual([
    ['stdout', 'promise-before'],
    ['stdout', 'promise-after'],
  ]);
});

test('synchronous evalVM rejects timer suspension promptly and keeps the runtime reusable', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    const started = performance.now();
    const suspended = interp.evalVM('(async/sleep 750) 99');
    const elapsed = performance.now() - started;
    const next = interp.evalVM('(+ 1 2)');
    return { suspended, elapsed, next };
  });

  expect(result.elapsed).toBeLessThan(250);
  expect(result.suspended.error).toContain('synchronous WebAssembly evaluation cannot suspend');
  expect(result.suspended.error).toContain('evalPromise');
  expect(result.next).toMatchObject({ value: '3', error: null });
});

test('synchronous eval rejected behind a promise debug barrier leaves no runnable root', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const paused = await interp.debugStartPromise('(define held 1)\n(+ held 1)', [2]);
    const rejected = interp.evalVM('(context/set :foreign-sync-root 99)');
    const stopAccepted = interp.debugStopPromise();
    const retiringBlocked = interp.debugStart('(context/set :retiring-debug-root-ran 1)', []);
    await new Promise((resolve) => setTimeout(resolve, 20));

    // A synchronous debugger drives the whole runtime. If evalVM admitted a
    // root before noticing the foreign debug barrier, that stranded root runs
    // here and mutates the shared context before the inspected expression.
    const entry = interp.debugStart('(context/get :foreign-sync-root)', []);
    const finished = interp.debugContinue();
    interp.debugStop();

    return { paused, rejected, stopAccepted, retiringBlocked, entry, finished };
  });

  expect(result.paused).toMatchObject({ status: 'stopped', line: 2 });
  expect(result.stopAccepted).toBe(true);
  expect(result.retiringBlocked).toMatchObject({ status: 'error' });
  expect(result.retiringBlocked.error).toContain('Promise-driven execution');
  expect(result.entry).toMatchObject({ status: 'stopped' });
  expect(result.finished).toMatchObject({ status: 'finished', value: null });
  expect(result.rejected.error).toContain('debugger is paused');
});

test('synchronous eval rejected behind a promise debug barrier cannot register a macro', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const paused = await interp.debugStartPromise('(+ 1 2)', []);
    const rejected = interp.evalVM('(defmacro leaked () 123)');
    const stopAccepted = interp.debugStopPromise();
    const probe = interp.evalVM('(leaked)');
    return { paused, rejected, stopAccepted, probe };
  });

  expect(result.paused).toMatchObject({ status: 'stopped' });
  expect(result.rejected.error).toContain('debugger is paused');
  expect(result.stopAccepted).toBe(true);
  expect(result.probe.value).toBeNull();
  expect(result.probe.error.toLowerCase()).toContain('unbound variable');
});

test('legacy debugger excludes same-interpreter Promise roots before admission', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const entry = interp.debugStart('(+ 1 2)', []);
    let promiseRoot: number | null = null;
    const rejected = await interp
      .evalPromise(
        '(context/set :legacy-promise-orphan 99)',
        (root: number) => { promiseRoot = root; },
      )
      .then(
        (value: string) => ({ value, error: null }),
        (error: Error) => ({ value: null, error: error.message }),
      );
    const promiseDebug = await interp.debugStartPromise(
      '(defmacro promise-debug-admission-leak () 7)\n(+ 1 2)',
      [],
    );

    const resumed = interp.debugContinue();
    await new Promise((resolve) => setTimeout(resolve, 20));

    // A fresh legacy drive would run a stranded root if the rejected Promise
    // had been submitted before the admission failure was discovered.
    const inspectionEntry = interp.debugStart('(context/get :legacy-promise-orphan)', []);
    const inspected = interp.debugContinue();
    interp.debugStop();
    const macroProbe = interp.evalVM('(promise-debug-admission-leak)');
    return {
      entry,
      promiseRoot,
      rejected,
      promiseDebug,
      resumed,
      inspectionEntry,
      inspected,
      macroProbe,
    };
  });

  expect(result.entry).toMatchObject({ status: 'stopped' });
  expect(result.promiseRoot).toBeNull();
  expect(result.rejected.value).toBeNull();
  expect(result.rejected.error).toContain('synchronous debugger');
  expect(result.promiseDebug).toMatchObject({ status: 'error' });
  expect(result.promiseDebug.error).toContain('synchronous debugger');
  expect(result.resumed).toMatchObject({ status: 'finished', value: '3' });
  expect(result.inspectionEntry).toMatchObject({ status: 'stopped' });
  expect(result.inspected).toMatchObject({ status: 'finished', value: null });
  expect(result.macroProbe.value).toBeNull();
  expect(result.macroProbe.error.toLowerCase()).toContain('unbound variable');
});

test('Promise root excludes same-interpreter legacy debugger before expansion', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    let promiseRoot: number | null = null;
    const pending = interp.evalPromise(
      '(async/sleep 30)\n' +
        '(context/set :promise-admission-runs (+ (or (context/get :promise-admission-runs) 0) 1))\n' +
        '"done"',
      (root: number) => { promiseRoot = root; },
    );
    const legacy = interp.debugStart(
      '(defmacro legacy-admission-leak () 7)\n' +
        '(context/set :rejected-legacy-debugger-ran 99)',
      [],
    );
    // Keep the pre-fix RED run finite. The corrected path returns an error
    // without creating a session, so this branch is then unreachable.
    if (legacy.status !== 'error') interp.debugStop();

    const promised = await pending.then(
      (value: string) => ({ value, error: null }),
      (error: Error) => ({ value: null, error: error.message }),
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    const runs = interp.evalVM('(context/get :promise-admission-runs)');
    const legacyMarker = interp.evalVM('(context/get :rejected-legacy-debugger-ran)');
    const macroProbe = interp.evalVM('(legacy-admission-leak)');
    return { promiseRoot, legacy, promised, runs, legacyMarker, macroProbe };
  });

  expect(result.promiseRoot).not.toBeNull();
  expect(result.legacy).toMatchObject({ status: 'error' });
  expect(result.legacy.error).toContain('Promise-driven execution');
  expect(result.promised).toEqual({ value: '"done"', error: null });
  expect(result.runs).toMatchObject({ value: '1', error: null });
  expect(result.legacyMarker).toMatchObject({ value: null, error: null });
  expect(result.macroProbe.value).toBeNull();
  expect(result.macroProbe.error.toLowerCase()).toContain('unbound variable');
});

test('legacy debugger on interpreter A does not block Promise root on interpreter B', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();

    const entryA = interpA.debugStart('(+ 1 2)', []);
    let rootB: number | null = null;
    const promisedB = await interpB
      .evalPromise(
        '(async/sleep 20)\n(context/set :interpreter-b-runs 1)\n"B-ok"',
        (root: number) => { rootB = root; },
      )
      .then(
        (value: string) => ({ value, error: null }),
        (error: Error) => ({ value: null, error: error.message }),
      );
    const activeABeforeContinue = interpA.debugIsActive();
    const finishedA = interpA.debugContinue();
    const probeB = interpB.evalVM('(context/get :interpreter-b-runs)');
    interpA.debugStop();
    return { entryA, rootB, promisedB, activeABeforeContinue, finishedA, probeB };
  });

  expect(result.entryA).toMatchObject({ status: 'stopped' });
  expect(result.rootB).not.toBeNull();
  expect(result.promisedB).toEqual({ value: '"B-ok"', error: null });
  expect(result.activeABeforeContinue).toBe(true);
  expect(result.finishedA).toMatchObject({ status: 'finished', value: '3' });
  expect(result.probeB).toMatchObject({ value: '1', error: null });
});

test('legacy debugger operations on interpreter B cannot mutate interpreter A session', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();

    const entryA = interpA.debugStart('(define owned 41)\n(+ owned 1)', []);
    const activeB = interpB.debugIsActive();
    const localsB = interpB.debugGetLocals();
    const stackB = interpB.debugGetStackTrace();
    const continueB = interpB.debugContinue();
    interpB.debugSetBreakpoints([2]);
    const startB = interpB.debugStart(
      '(defmacro foreign-legacy-admission-leak () 9)\n(+ 1 2)',
      [],
    );
    const activeAAfterBOps = interpA.debugIsActive();
    const finishedA = interpA.debugContinue();
    interpA.debugStop();
    interpB.debugStop();
    const macroProbeB = interpB.evalVM('(foreign-legacy-admission-leak)');

    const interpA2 = new mod.SemaInterpreter();
    const entryA2 = interpA2.debugStart('(+ 1 2)', []);
    interpB.debugStop();
    const activeA2AfterBStop = interpA2.debugIsActive();
    const finishedA2 = interpA2.debugContinue();
    interpA2.debugStop();

    return {
      entryA,
      activeB,
      localsB,
      stackB,
      continueB,
      startB,
      activeAAfterBOps,
      finishedA,
      macroProbeB,
      entryA2,
      activeA2AfterBStop,
      finishedA2,
    };
  });

  expect(result.entryA).toMatchObject({ status: 'stopped' });
  expect(result.activeB).toBe(false);
  expect(result.localsB).toBeNull();
  expect(result.stackB).toEqual([]);
  expect(result.continueB).toMatchObject({ status: 'error' });
  expect(result.startB).toMatchObject({ status: 'error' });
  expect(result.startB.error).toContain('another interpreter');
  expect(result.activeAAfterBOps).toBe(true);
  expect(result.finishedA).toMatchObject({ status: 'finished', value: '42' });
  expect(result.macroProbeB.value).toBeNull();
  expect(result.macroProbeB.error.toLowerCase()).toContain('unbound variable');
  expect(result.entryA2).toMatchObject({ status: 'stopped' });
  expect(result.activeA2AfterBStop).toBe(true);
  expect(result.finishedA2).toMatchObject({ status: 'finished', value: '3' });
});

test('legacy debugStart reserves its owner across macro-expansion JS re-entry', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();
    let nestedRoot: number | null = null;
    let nestedPromise: Promise<{ value: string | null; error: string | null }> | undefined;
    let foreignStart: Record<string, unknown> | null = null;
    let activeDuringStart: boolean | null = null;
    let continueDuringStart: Record<string, unknown> | null = null;
    let localsDuringStart: unknown = 'not-called';
    let stackDuringStart: unknown = 'not-called';

    interpA.registerFunction('reenter-debug-start', () => {
      nestedPromise = interpA
        .evalPromise(
          '(context/set :macro-reentrant-promise-ran 1)',
          (root: number) => { nestedRoot = root; },
        )
        .then(
          (value: string) => ({ value, error: null }),
          (error: Error) => ({ value: null, error: error.message }),
        );
      foreignStart = interpB.debugStart(
        '(defmacro foreign-start-admission-leak () 9)\n(+ 1 2)',
        [],
      );
      activeDuringStart = interpA.debugIsActive();
      continueDuringStart = interpA.debugContinue();
      localsDuringStart = interpA.debugGetLocals();
      stackDuringStart = interpA.debugGetStackTrace();
      interpA.debugSetBreakpoints([99]);
      return null;
    });

    const outer = interpA.debugStart(
      "(defmacro trigger-reentrant-start () (reenter-debug-start) '(+ 1 2))\n" +
        '(trigger-reentrant-start)',
      [],
    );
    const nested = nestedPromise
      ? await nestedPromise
      : { value: null, error: 'macro callback did not return' };
    const finished = outer.status === 'stopped' ? interpA.debugContinue() : outer;
    await new Promise((resolve) => setTimeout(resolve, 20));
    const nestedProbe = interpA.evalVM('(context/get :macro-reentrant-promise-ran)');
    const foreignMacroProbe = interpB.evalVM('(foreign-start-admission-leak)');
    interpA.debugStop();
    interpB.debugStop();
    return {
      nestedRoot,
      nested,
      foreignStart,
      activeDuringStart,
      continueDuringStart,
      localsDuringStart,
      stackDuringStart,
      outer,
      finished,
      nestedProbe,
      foreignMacroProbe,
    };
  });

  expect(result.nestedRoot).toBeNull();
  expect(result.nested.value).toBeNull();
  expect(result.nested.error).toContain('synchronous debugger');
  expect(result.foreignStart).toMatchObject({ status: 'error' });
  expect(result.activeDuringStart).toBe(true);
  expect(result.continueDuringStart).toMatchObject({ status: 'error' });
  expect(result.localsDuringStart).toBeNull();
  expect(result.stackDuringStart).toEqual([]);
  expect(result.outer).toMatchObject({ status: 'stopped' });
  expect(result.finished).toMatchObject({ status: 'finished', value: '3' });
  expect(result.nestedProbe).toMatchObject({ value: null, error: null });
  expect(result.foreignMacroProbe.value).toBeNull();
  expect(result.foreignMacroProbe.error.toLowerCase()).toContain('unbound variable');
});

test('legacy debug drive permits callback re-entry without borrowing or orphaning roots', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();
    let nestedRoot: number | null = null;
    let nestedPromise: Promise<{ value: string | null; error: string | null }> | undefined;
    let foreignStart: Record<string, unknown> | null = null;
    let activeDuringDrive: boolean | null = null;
    let continueDuringDrive: Record<string, unknown> | null = null;
    let pollDuringDrive: Record<string, unknown> | null = null;
    let localsDuringDrive: unknown = 'not-called';
    let stackDuringDrive: unknown = 'not-called';

    interpA.registerFunction('reenter-debug-drive', () => {
      nestedPromise = interpA
        .evalPromise(
          '(context/set :drive-reentrant-promise-ran 1)',
          (root: number) => { nestedRoot = root; },
        )
        .then(
          (value: string) => ({ value, error: null }),
          (error: Error) => ({ value: null, error: error.message }),
        );
      foreignStart = interpB.debugStart('(context/set :foreign-drive-debugger-ran 1)', []);
      activeDuringDrive = interpA.debugIsActive();
      continueDuringDrive = interpA.debugContinue();
      pollDuringDrive = interpA.debugPoll();
      localsDuringDrive = interpA.debugGetLocals();
      stackDuringDrive = interpA.debugGetStackTrace();
      interpA.debugSetBreakpoints([99]);
      interpA.debugStop();
      return 41;
    });

    const outer = interpA.debugStart(
      '(reenter-debug-drive)\n(context/set :outer-drive-body-ran 1)\n42',
      [2],
    );
    const nested = nestedPromise
      ? await nestedPromise
      : { value: null, error: 'drive callback did not return' };
    await new Promise((resolve) => setTimeout(resolve, 20));
    const promiseProbe = interpA.evalVM('(context/get :drive-reentrant-promise-ran)');
    const outerProbe = interpA.evalVM('(context/get :outer-drive-body-ran)');
    const foreignProbe = interpB.evalVM('(context/get :foreign-drive-debugger-ran)');
    const activeAfter = interpA.debugIsActive();
    const reusableEntry = interpA.debugStart('(+ 1 2)', []);
    const reusableFinished = interpA.debugContinue();
    interpA.debugStop();
    interpB.debugStop();
    return {
      nestedRoot,
      nested,
      foreignStart,
      activeDuringDrive,
      continueDuringDrive,
      pollDuringDrive,
      localsDuringDrive,
      stackDuringDrive,
      outer,
      promiseProbe,
      outerProbe,
      foreignProbe,
      activeAfter,
      reusableEntry,
      reusableFinished,
    };
  });

  expect(result.nestedRoot).toBeNull();
  expect(result.nested.value).toBeNull();
  expect(result.nested.error).toContain('synchronous debugger');
  expect(result.foreignStart).toMatchObject({ status: 'error' });
  expect(result.activeDuringDrive).toBe(true);
  expect(result.continueDuringDrive).toMatchObject({ status: 'error' });
  expect(result.pollDuringDrive).toMatchObject({ status: 'error' });
  expect(result.localsDuringDrive).toBeNull();
  expect(result.stackDuringDrive).toEqual([]);
  expect(result.outer).toMatchObject({ status: 'error' });
  expect(result.promiseProbe).toMatchObject({ value: null, error: null });
  expect(result.outerProbe).toMatchObject({ value: null, error: null });
  expect(result.foreignProbe).toMatchObject({ value: null, error: null });
  expect(result.activeAfter).toBe(false);
  expect(result.reusableEntry).toMatchObject({ status: 'stopped' });
  expect(result.reusableFinished).toMatchObject({ status: 'finished', value: '3' });
});

test('evalPromise reserves admission across macro-expansion JS re-entry', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();
    let sameStart: Record<string, unknown> | null = null;
    let sameActive: boolean | null = null;
    let foreignStart: Record<string, unknown> | null = null;

    interpA.registerFunction('reenter-promise-eval-preparation', () => {
      sameStart = interpA.debugStart(
        '(defmacro promise-eval-legacy-leak () 17)\n(+ 1 2)',
        [],
      );
      sameActive = interpA.debugIsActive();
      // Keep the pre-fix RED run finite. A reservation makes this unreachable.
      if (sameStart.status !== 'error') interpA.debugStop();
      foreignStart = interpB.debugStart('(+ 20 22)', []);
      return null;
    });

    let outerRoot: number | null = null;
    const outer = await interpA
      .evalPromise(
        "(defmacro trigger-promise-eval-preparation () (reenter-promise-eval-preparation) '(begin (context/set :promise-eval-outer-runs 1) 42))\n" +
          '(trigger-promise-eval-preparation)',
        (root: number) => { outerRoot = root; },
      )
      .then(
        (value: string) => ({ value, error: null }),
        (error: Error) => ({ value: null, error: error.message }),
      );
    const foreignFinished = foreignStart?.status === 'stopped'
      ? interpB.debugContinue()
      : foreignStart;
    interpB.debugStop();

    const outerRuns = interpA.evalVM('(context/get :promise-eval-outer-runs)');
    const macroProbe = interpA.evalVM('(promise-eval-legacy-leak)');
    const reusableEntry = interpA.debugStart('(+ 1 2)', []);
    const reusableFinished = interpA.debugContinue();
    interpA.debugStop();
    return {
      sameStart,
      sameActive,
      foreignStart,
      foreignFinished,
      outerRoot,
      outer,
      outerRuns,
      macroProbe,
      reusableEntry,
      reusableFinished,
    };
  });

  expect(result.sameStart).toMatchObject({ status: 'error' });
  expect(result.sameStart.error).toContain('Promise-driven execution');
  expect(result.sameActive).toBe(false);
  expect(result.foreignStart).toMatchObject({ status: 'stopped' });
  expect(result.foreignFinished).toMatchObject({ status: 'finished', value: '42' });
  expect(result.outerRoot).not.toBeNull();
  expect(result.outer).toEqual({ value: '42', error: null });
  expect(result.outerRuns).toMatchObject({ value: '1', error: null });
  expect(result.macroProbe.value).toBeNull();
  expect(result.macroProbe.error.toLowerCase()).toContain('unbound variable');
  expect(result.reusableEntry).toMatchObject({ status: 'stopped' });
  expect(result.reusableFinished).toMatchObject({ status: 'finished', value: '3' });
});

test('debugStartPromise reserves admission across macro-expansion JS re-entry', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();
    let sameStart: Record<string, unknown> | null = null;
    let sameActive: boolean | null = null;
    let foreignStart: Record<string, unknown> | null = null;

    interpA.registerFunction('reenter-promise-debug-preparation', () => {
      sameStart = interpA.debugStart(
        '(defmacro promise-debug-legacy-leak () 23)\n(+ 1 2)',
        [],
      );
      sameActive = interpA.debugIsActive();
      // Keep the pre-fix RED run finite. A reservation makes this unreachable.
      if (sameStart.status !== 'error') interpA.debugStop();
      foreignStart = interpB.debugStart('(+ 39 3)', []);
      return null;
    });

    const outerEntry = await interpA.debugStartPromise(
      "(defmacro trigger-promise-debug-preparation () (reenter-promise-debug-preparation) '(+ 40 2))\n" +
        '(trigger-promise-debug-preparation)',
      [],
    );
    const outerFinished = outerEntry.status === 'stopped'
      ? await interpA.debugContinuePromise()
      : outerEntry;
    const foreignFinished = foreignStart?.status === 'stopped'
      ? interpB.debugContinue()
      : foreignStart;
    interpB.debugStop();

    const macroProbe = interpA.evalVM('(promise-debug-legacy-leak)');
    const reusableEntry = interpA.debugStart('(+ 1 2)', []);
    const reusableFinished = interpA.debugContinue();
    interpA.debugStop();
    return {
      sameStart,
      sameActive,
      foreignStart,
      foreignFinished,
      outerEntry,
      outerFinished,
      macroProbe,
      reusableEntry,
      reusableFinished,
    };
  });

  expect(result.sameStart).toMatchObject({ status: 'error' });
  expect(result.sameStart.error).toContain('Promise-driven execution');
  expect(result.sameActive).toBe(false);
  expect(result.foreignStart).toMatchObject({ status: 'stopped' });
  expect(result.foreignFinished).toMatchObject({ status: 'finished', value: '42' });
  expect(result.outerEntry).toMatchObject({ status: 'stopped' });
  expect(result.outerFinished).toMatchObject({ status: 'finished', value: '42' });
  expect(result.macroProbe.value).toBeNull();
  expect(result.macroProbe.error.toLowerCase()).toContain('unbound variable');
  expect(result.reusableEntry).toMatchObject({ status: 'stopped' });
  expect(result.reusableFinished).toMatchObject({ status: 'finished', value: '3' });
});

test('a detached foreign timer does not delay promise-root deadlock settlement', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const foreign = interp.evalVM('(async/spawn (fn () (async/sleep 800)))');
    const started = performance.now();
    const deadlocked = await interp.evalPromise('(channel/recv (channel/new 1))').then(
      (value: string) => ({ value, error: null }),
      (error: Error) => ({ value: null, error: error.message }),
    );
    return { foreign, deadlocked, elapsed: performance.now() - started };
  });

  expect(result.foreign.error).toBeNull();
  expect(result.deadlocked.value).toBeNull();
  expect(result.deadlocked.error).toContain('deadlock');
  expect(result.elapsed).toBeLessThan(250);
});

test('a detached foreign external wait does not suppress promise-root deadlock settlement', async ({ page }) => {
  await page.route('**/foreign-external-wait', async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 800));
    await route.fulfill({ status: 200, body: 'ok' });
  });
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    const foreign = await interp.evalPromise(
      `(let ((task (async/spawn (fn () (http/get "${location.origin}/foreign-external-wait")))))
         (async/sleep 20)
         task)`,
    );
    const started = performance.now();
    const deadlocked = await interp.evalPromise('(channel/recv (channel/new 1))').then(
      (value: string) => ({ value, error: null }),
      (error: Error) => ({ value: null, error: error.message }),
    );
    return { foreign, deadlocked, elapsed: performance.now() - started };
  });

  expect(result.foreign).toBe('<async-promise>');
  expect(result.deadlocked.value).toBeNull();
  expect(result.deadlocked.error).toContain('deadlock');
  expect(result.elapsed).toBeLessThan(250);
});

test('synchronous debugger rejects suspension and clears only its session', async ({ page }) => {
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the dev server at runtime, not by tsc
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    const entry = interp.debugStart('(async/sleep 750)\n99', []);
    const started = performance.now();
    const suspended = interp.debugContinue();
    const elapsed = performance.now() - started;
    const activeAfterError = interp.debugIsActive();
    const next = interp.debugStart('(+ 1 2)', []);
    interp.debugStop();
    return { entry, suspended, elapsed, activeAfterError, next };
  });

  expect(result.entry.status).toBe('stopped');
  expect(result.elapsed).toBeLessThan(250);
  expect(result.suspended.status).toBe('error');
  expect(result.suspended.error).toContain('debugStartPromise');
  expect(result.activeAfterError).toBe(false);
  expect(result.next.status).toBe('stopped');
});

// ── Gate (f), un-scoped (P6-3 step 5 —
// `docs/plans/archive/2026-07-16-wasm-promise-driven-roots.md`): the replay loops and
// the JS-side SAB/`legacySab` fallback are deleted, so this scans the full
// production sources rather than one reachable branch. Promise-driven HTTP,
// sleep, and debugging now own every admissible suspension; synchronous WASM
// entry points reject suspension instead of retaining a blocking fallback.
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

test('the WASM crate has no replay cache, synchronous XHR, or Atomics host adapter', () => {
  const libSrc = readFileSync(path.join(REPO_ROOT, 'crates/sema-wasm/src/lib.rs'), 'utf8');
  const cargoSrc = readFileSync(path.join(REPO_ROOT, 'crates/sema-wasm/Cargo.toml'), 'utf8');
  for (const marker of [
    'HTTP_AWAIT_MARKER',
    'HTTP_CACHE',
    'DEBUG_HTTP_REPLAY_ARMED',
    'debugPerformFetch',
    'installAtomicsSleep',
    'worker_atomics_sleep',
    'worker_check_interrupt',
    'XmlHttpRequest',
  ]) {
    expect(libSrc).not.toContain(marker);
  }
  expect(cargoSrc).not.toContain('XmlHttpRequest');
});

test('the RustEmbedded sema web artifacts omit retired blocking exports and host imports', () => {
  const assetDir = path.join(REPO_ROOT, 'crates/sema/src/web/assets');
  const glue = readFileSync(path.join(assetDir, 'sema_wasm.js'), 'utf8');
  for (const marker of [
    'debugPerformFetch',
    'installAtomicsSleep',
    'XMLHttpRequest',
    'Atomics.wait',
    'HTTP_AWAIT_MARKER',
  ]) {
    expect(glue).not.toContain(marker);
  }

  const wasm = new WebAssembly.Module(readFileSync(path.join(assetDir, 'sema_wasm_bg.wasm')));
  const exports = WebAssembly.Module.exports(wasm).map(({ name }) => name);
  expect(exports).not.toContain('semainterpreter_debugPerformFetch');
  expect(exports).not.toContain('semainterpreter_installAtomicsSleep');
});

test('the shipped default worker protocol never reaches legacy Atomics/replay code (SAB deleted entirely)', async () => {
  const workerSrc = readFileSync(path.join(__dirname, '..', 'dist', 'sema-worker.js'), 'utf8');
  const appSrc = readFileSync(path.join(__dirname, '..', 'dist', 'app.js'), 'utf8');

  // Retired replay markers must not appear in either shipped JS bundle.
  for (const marker of ['HTTP_AWAIT_MARKER', 'MAX_REPLAYS', 'legacySab', 'SharedArrayBuffer(']) {
    expect(workerSrc).not.toContain(marker);
    expect(appSrc).not.toContain(marker);
  }
  // No blocking Atomics wait/notify call exists in the shipped worker or app.
  expect(workerSrc).not.toMatch(/Atomics\.(wait|store|notify)\(/);
  expect(appSrc).not.toMatch(/Atomics\.(wait|store|notify)\(/);
  expect(workerSrc).not.toContain('installAtomicsSleep');
  expect(appSrc).not.toContain('installAtomicsSleep');

  // The default protocol's own reachable entry points call the new seam.
  expect(workerSrc).toContain('evalPromise');
  expect(workerSrc).toContain('cancelRoot');
});
