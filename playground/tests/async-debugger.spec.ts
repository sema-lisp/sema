import { test, expect, Page } from '@playwright/test';
import { toggleBreakpoint, getCurrentDebugLine } from './gutter';

// E2E gate (Slice 2): breakpoints INSIDE async tasks STOP + CONTINUE in the
// cooperative WASM playground debugger. Modeled on debugger.spec.ts — same UI
// flow + selectors. We inject our OWN known programs (not a built-in example) so
// the breakpoint line is deterministic.

async function waitForReady(page: Page) {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });
}

async function setEditorCode(page: Page, code: string) {
  await page.getByTestId('editor').fill(code);
}

/** Get all error output. */
async function getErrors(page: Page): Promise<string[]> {
  return page.getByTestId('output-error').allTextContents();
}

/** Wait for the debugger to pause (status bar shows "Paused at line ..."). */
async function waitForPaused(page: Page, timeout = 8000) {
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent?.startsWith('Paused'),
    { timeout }
  );
}

/** Wait for the debugger to return to idle (run finished). */
async function waitForIdle(page: Page, timeout = 12000) {
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent === 'Ready',
    { timeout }
  );
}

async function getStatus(page: Page): Promise<string> {
  return await page.getByTestId('status').textContent() ?? '';
}

async function getOutputLines(page: Page): Promise<string[]> {
  return page.getByTestId('output-line').allTextContents();
}

/** Click a debug control (by testid) and wait until the debugger is paused
 *  again or idle. */
async function clickAndSettle(page: Page, testId: string) {
  await page.getByTestId(testId).click();
  await page.waitForFunction(
    () => {
      const s = document.getElementById('status')?.textContent ?? '';
      return s.startsWith('Paused') || s === 'Ready';
    },
    { timeout: 8000 }
  );
}

/** Repeatedly click `testId` (a step/continue control), recording the line at
 *  each stop, until the program goes idle or `maxSteps` is reached. */
async function recordStops(page: Page, testId: string, maxSteps = 12): Promise<number[]> {
  const lines: number[] = [];
  const first = await getCurrentDebugLine(page);
  if (first !== null) lines.push(first);
  for (let i = 0; i < maxSteps; i++) {
    if ((await getStatus(page)) === 'Ready') break;
    await clickAndSettle(page, testId);
    if ((await getStatus(page)) === 'Ready') break;
    const l = await getCurrentDebugLine(page);
    if (l !== null) lines.push(l);
  }
  return lines;
}

/** Continue repeatedly until the program goes idle — handles a breakpoint that
 *  re-triggers (e.g. inside a loop) and a step that already finished the run. */
async function driveToIdle(page: Page, maxContinues = 25) {
  for (let i = 0; i < maxContinues; i++) {
    if ((await getStatus(page)) === 'Ready') return;
    await clickAndSettle(page, 'dbg-continue');
  }
  await waitForIdle(page);
}

test.describe('Async debugger (cooperative WASM)', () => {
  test.beforeEach(async ({ page }) => {
    await waitForReady(page);
  });

  test('breakpoint inside an async task: stops on the task line, then Continue finishes', async ({
    page,
  }) => {
    // Line 2 is `(+ 1 2)` — runs ONLY inside the spawned task body. Before
    // Slice 2 the cooperative debugger swallowed this stop and ran to the end.
    const code = '(define p (async/spawn (fn ()\n  (+ 1 2))))\n(await p)';
    await setEditorCode(page, code);

    // Set the breakpoint on line 2 and start debugging.
    await toggleBreakpoint(page, 2);
    await page.getByTestId('debug-btn').click();

    // Must pause INSIDE the task, on line 2.
    await waitForPaused(page);
    expect(await getCurrentDebugLine(page)).toBe(2);

    // Continue → the task + the await must run to completion.
    await page.getByTestId('dbg-continue').click();
    await waitForIdle(page);

    expect((await getErrors(page)).join('\n')).not.toContain('scheduler');
  });

  test('breakpoint inside the second of two async tasks: pauses at the known line', async ({
    page,
  }) => {
    // 1  (define a (async/spawn (fn ()
    // 2    (* 2 3))))
    // 3  (define b (async/spawn (fn ()
    // 4    (+ 10 20))))      <- breakpoint here, inside task b only
    // 5  (async/all (list a b))
    const code =
      '(define a (async/spawn (fn ()\n  (* 2 3))))\n' +
      '(define b (async/spawn (fn ()\n  (+ 10 20))))\n' +
      '(async/all (list a b))';
    await setEditorCode(page, code);

    await toggleBreakpoint(page, 4);
    await page.getByTestId('debug-btn').click();

    await waitForPaused(page);
    // The stop must be on line 4 (task b's body), not line 2 (task a) or the
    // top-level async/all.
    expect(await getCurrentDebugLine(page)).toBe(4);

    await page.getByTestId('dbg-continue').click();
    await waitForIdle(page);

    expect((await getErrors(page)).join('\n')).not.toContain('scheduler');
  });

  // ── Comprehensive stress tests of the debug mechanics ──────────────────

  test('multiple breakpoints (main + inside a task): Continue hits each in order', async ({
    page,
  }) => {
    // 1 (define a 10)
    // 2 (define p (async/spawn (fn ()
    // 3   (define x 1)
    // 4   (define y 2)        <- bp (inside the task)
    // 5   (+ x y))))
    // 6 (define b (await p))
    // 7 (println (+ a b))     <- bp (main, after the task)
    const code =
      '(define a 10)\n' +
      '(define p (async/spawn (fn ()\n' +
      '  (define x 1)\n' +
      '  (define y 2)\n' +
      '  (+ x y))))\n' +
      '(define b (await p))\n' +
      '(println (+ a b))';
    await setEditorCode(page, code);

    await toggleBreakpoint(page, 1);
    await toggleBreakpoint(page, 4);
    await toggleBreakpoint(page, 7);
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);

    const stops = await recordStops(page, 'dbg-continue');
    console.log('multi-bp Continue stops:', stops);
    // Each breakpoint is hit, in source order: main(1) → task body(4) → main(7).
    expect(stops).toEqual([1, 4, 7]);
    await waitForIdle(page);
    expect(await getOutputLines(page)).toContain('13');
    expect((await getErrors(page)).join('\n')).not.toContain('scheduler');
  });

  test('step into: descends into a (sync) function call', async ({ page }) => {
    const code = '(define (add a b) (+ a b))\n(define r (add 3 4))\n(println r)';
    await setEditorCode(page, code);
    await page.getByTestId('debug-btn').click(); // no breakpoints → stop on entry
    await waitForPaused(page);

    const lines = await recordStops(page, 'dbg-step-into');
    console.log('step-into lines:', lines);
    // Step-into must DESCEND into add's body (which lives on line 1), so line 1 is
    // visited more than once (as the definition AND as the body during the call).
    expect(lines.filter(l => l === 1).length).toBeGreaterThanOrEqual(2);
    await waitForIdle(page);
    expect(await getOutputLines(page)).toContain('7');
  });

  test('step over: does NOT descend into a function call', async ({ page }) => {
    const code = '(define (add a b) (+ a b))\n(define r (add 3 4))\n(println r)';
    await setEditorCode(page, code);
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);

    const lines = await recordStops(page, 'dbg-step-over');
    console.log('step-over lines:', lines);
    // Stays at the top level — line 1 is visited exactly once (the definition);
    // the call on line 2 is stepped OVER, never descending back into line 1.
    expect(lines.filter(l => l === 1).length).toBe(1);
    expect(lines[lines.length - 1]).toBe(3);
    await waitForIdle(page);
    expect(await getOutputLines(page)).toContain('7');
  });

  test('step out: leaves the function frame (returns to caller or finishes)', async ({ page }) => {
    // 1 (define (f x)
    // 2   (* x 2))      <- bp inside f
    // 3 (define r (f 5))
    // 4 (define s (+ r 1))
    // 5 (println s)
    const code = '(define (f x)\n  (* x 2))\n(define r (f 5))\n(define s (+ r 1))\n(println s)';
    await setEditorCode(page, code);
    await toggleBreakpoint(page, 2);
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    expect(await getCurrentDebugLine(page)).toBe(2);

    await clickAndSettle(page, 'dbg-step-out');
    const after = await getCurrentDebugLine(page);
    const status = await getStatus(page);
    console.log('after step-out: line', after, 'status', status);
    // The whole point of step-out: we are NO LONGER inside f's body (line 2) —
    // either back in the caller (line > 2) or the run finished.
    expect(after === null || (after !== null && after > 2)).toBeTruthy();
    await driveToIdle(page);
    expect(await getOutputLines(page)).toContain('11'); // r=10, s=11
  });

  test('step into advances line-by-line WITHIN an async task body', async ({ page }) => {
    // 1 (define p (async/spawn (fn ()
    // 2   (define x 1)   <- bp
    // 3   (define y 2)
    // 4   (+ x y))))
    // 5 (await p)
    const code =
      '(define p (async/spawn (fn ()\n' +
      '  (define x 1)\n' +
      '  (define y 2)\n' +
      '  (+ x y))))\n' +
      '(await p)';
    await setEditorCode(page, code);
    await toggleBreakpoint(page, 2);
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    expect(await getCurrentDebugLine(page)).toBe(2);

    // Stepping inside the stopped task advances within the task (2 → 3 → 4).
    await clickAndSettle(page, 'dbg-step-into');
    const l3 = await getCurrentDebugLine(page);
    console.log('within-task step 1 → line', l3);
    expect(l3).toBe(3);
    await clickAndSettle(page, 'dbg-step-into');
    const l4 = await getCurrentDebugLine(page);
    console.log('within-task step 2 → line', l4);
    expect(l4).toBe(4);

    await page.getByTestId('dbg-continue').click();
    await waitForIdle(page);
    expect((await getErrors(page)).join('\n')).not.toContain('scheduler');
  });

  test('variant: channel pipeline with a breakpoint resumes and produces output', async ({
    page,
  }) => {
    // Async via channels + a worker task; breakpoint inside the worker loop.
    // 1 (define ch (channel/new 4))
    // 2 (channel/send ch 10)
    // 3 (channel/send ch 20)
    // 4 (channel/close ch)
    // 5 (define total (await (async (let loop ((s 0))
    // 6   (let ((v (channel/recv ch)))
    // 7     (if (nil? v) s (loop (+ s v))))))))
    // 8 (println total)
    const code =
      '(define ch (channel/new 4))\n' +
      '(channel/send ch 10)\n' +
      '(channel/send ch 20)\n' +
      '(channel/close ch)\n' +
      '(define total (await (async (let loop ((s 0))\n' +
      '  (let ((v (channel/recv ch)))\n' +
      '    (if (nil? v) s (loop (+ s v))))))))\n' +
      '(println total)';
    await setEditorCode(page, code);
    await toggleBreakpoint(page, 6); // inside the worker task (runs only via await)
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    expect(await getCurrentDebugLine(page)).toBe(6);

    // The breakpoint sits inside the worker's recv loop, so it re-triggers each
    // iteration — Continue through all of them until the run finishes.
    await driveToIdle(page);
    expect(await getOutputLines(page)).toContain('30');
    expect((await getErrors(page)).join('\n')).not.toContain('scheduler');
  });
});
