// P6-3: WASM Promise-driven roots — acceptance gate (SCAFFOLD, not yet active).
//
// These tests pin the real-browser oracle for the unified async runtime landing
// described in docs/plans/2026-07-16-wasm-promise-driven-roots.md. They are
// `test.fixme` because the Promise-driven `eval()` API they assert against does
// NOT exist yet — the shipped mechanism is still replay-with-cache (HTTP) plus
// Atomics.wait (sleep). Un-`fixme` each test only when the corresponding piece
// of the Promise/macrotask/External-via-JS-callback design lands, then run:
//
//   jake pg.build && jake test.playground-e2e
//
// against a real headless Chromium. A passing run of (a) and (b) is the ONLY
// evidence that authorizes deleting the replay/Atomics machinery.
import { test, expect, type Page } from '@playwright/test';

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
test.fixme('http/get resolves via Promise with the body executing once (no replay)', async ({ page }) => {
  await page.goto('/');
  await setCode(
    page,
    [
      '(println "BEFORE-FETCH-MARKER")',
      '(def resp (http/get "https://httpbin.org/get"))',
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
test.fixme('async/sleep completes via setTimeout without blocking the page', async ({ page }) => {
  await page.goto('/');
  await setCode(
    page,
    [
      '(println "START")',
      '(async/sleep 250)',
      '(println "AFTER-SLEEP")',
    ].join('\n'),
  );
  await page.getByTestId('run-btn').click();

  // While the sleep is pending the page must stay responsive (main thread not
  // blocked by Atomics.wait): a trivial DOM interaction still resolves.
  await page.waitForFunction(() => document.body.innerText.includes('START'), {
    timeout: 5_000,
  });
  const responsiveWhilePending = await page.evaluate(
    () => new Promise<boolean>((r) => setTimeout(() => r(true), 0)),
  );
  expect(responsiveWhilePending).toBe(true);

  await page.waitForFunction(
    () => document.body.innerText.includes('AFTER-SLEEP'),
    { timeout: 10_000 },
  );
});

// ── Gate (c): two concurrent roots settle fairly with distinct identity ──────
test.fixme('two concurrent eval roots stay pending and settle fairly', async ({ page }) => {
  await page.goto('/');
  // Requires the root-aware playground protocol (multiple pending eval requests
  // over the worker) — pins that two evaluations interleave and both complete.
  expect(true).toBe(true);
});

// ── Gate (d): Stop cancels one exact root; the other continues ───────────────
test.fixme('Stop cancels the exact RootId via RuntimeCommandHandle::cancel_root', async ({ page }) => {
  await page.goto('/');
  // Requires cancel routed through the runtime command handle (not the SAB
  // cancel flag). Pins that cancelling root A leaves root B running to completion.
  expect(true).toBe(true);
});
