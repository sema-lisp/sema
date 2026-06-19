import { test, expect, Page } from '@playwright/test';

// Example file names as they appear in the sidebar tree
const EXAMPLES = [
  'hello.sema',
  'fibonacci.sema',
  'fizzbuzz.sema',
  'quicksort.sema',
  'closures.sema',
  'map-filter.sema',
  'strings.sema',
  'macros.sema',
  'maze.sema',
  'mandelbrot.sema',
  'perlin-noise.sema',
  'game-of-life.sema',
  'ascii-art.sema',
  // Concurrency examples — exercise the async scheduler + channels in WASM
  // (where async/sleep is a no-op yield), stressing fan-out/pipeline/fan-in.
  'channels.sema',
  'parallel-tasks.sema',
  'timeout.sema',
  'worker-pool.sema',
  'pipeline.sema',
  'fan-in.sema',
];

/** Wait for the WASM module to be ready. */
async function waitForReady(page: Page) {
  await page.goto('/');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 15000 });
}

/** Type code into the editor, replacing existing content. */
async function setEditorCode(page: Page, code: string) {
  await page.getByTestId('editor').fill(code);
}

/** Click Run and wait for the timing line to appear. */
async function clickRunAndWait(page: Page) {
  await page.getByTestId('run-btn').click();
  await page.waitForSelector('#output .output-timing', { timeout: 30000 });
}

test.beforeEach(async ({ page }) => {
  await waitForReady(page);
});

/** Expand every sidebar category so its example buttons are clickable.
 *  Only "Getting Started" is expanded by default; other categories collapse. */
async function expandAllCategories(page: Page) {
  await page.evaluate(() => {
    document
      .querySelectorAll('.tree-items.collapsed')
      .forEach(el => el.classList.remove('collapsed'));
  });
}

// ── Example smoke tests ──

for (const name of EXAMPLES) {
  test(`example: ${name}`, async ({ page }) => {
    // Expand all categories, then click the example button in the sidebar tree
    await expandAllCategories(page);
    await page.click(`.tree-file:text("${name}")`);

    // Verify editor has content
    const editorValue = await page.getByTestId('editor').inputValue();
    expect(editorValue.length).toBeGreaterThan(10);

    // Click Run
    await clickRunAndWait(page);

    // Check there's no error
    const errorEl = await page.$('#output .output-error');
    if (errorEl) {
      const errorText = await errorEl.textContent();
      throw new Error(`Example "${name}" produced error: ${errorText}`);
    }

    // Check we got some output (either output lines or a value)
    const outputLines = await page.$$('#output .output-line');
    const valueLines = await page.$$('#output .output-value');
    expect(outputLines.length + valueLines.length).toBeGreaterThan(0);

    // Verify timing shows the bytecode VM (the sole evaluator)
    const timing = await page.$eval('#output .output-timing', el => el.textContent);
    expect(timing).toContain('bytecode VM');
  });
}

test('whitespace preserved in output', async ({ page }) => {
  // Use the Maze example which relies on whitespace alignment
  await expandAllCategories(page);
  await page.click('.tree-file:text("maze.sema")');
  await clickRunAndWait(page);

  // Check that output lines have white-space: pre
  const style = await page.$eval('.output-line', (el) =>
    window.getComputedStyle(el).whiteSpace
  );
  expect(style).toBe('pre');
});

// ── VM evaluation tests (bytecode VM is the sole evaluator) ──

test('runs code with the bytecode VM', async ({ page }) => {
  await setEditorCode(page, '(+ 1 2)');
  await clickRunAndWait(page);

  // Check result
  const value = await page.$eval('#output .output-value', el => el.textContent);
  expect(value).toContain('3');

  // Verify timing reports the bytecode VM
  const timing = await page.$eval('#output .output-timing', el => el.textContent);
  expect(timing).toContain('bytecode VM');
});

test('async/sleep ordering works in WASM (virtual clock)', async ({ page }) => {
  // Regression guard for the virtual clock: in WASM async/sleep has no real
  // delay, but shorter sleeps must still wake before longer ones. Tasks are
  // spawned c/a/b but sleep 30/10/20 — output must be a, b, c.
  const code = `(async/all
  (list (async (async/sleep 30) (println "c"))
        (async (async/sleep 10) (println "a"))
        (async (async/sleep 20) (println "b"))))`;
  await setEditorCode(page, code);
  await clickRunAndWait(page);
  const lines = await page.$$eval('#output .output-line', els =>
    els.map(el => el.textContent)
  );
  expect(lines).toEqual(['a', 'b', 'c']);
});

test('worker path: async/sleep paces in real wall-clock while the UI stays responsive', async ({ page }) => {
  // Opt into the worker eval path (?worker). Requires cross-origin isolation,
  // which the dev server provides via serve.json (COOP/COEP).
  await page.goto('/?worker');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 20000 });

  // The worker path must actually be active (cross-origin isolated + SAB),
  // otherwise this would silently fall back to the instant main-thread path.
  const isolated = await page.evaluate(
    () => self.crossOriginIsolated === true && 'SharedArrayBuffer' in globalThis
  );
  expect(isolated).toBe(true);

  // Three concurrent sleeps (100/200/300ms) then a final print. On the worker
  // these are REAL waits, so the run takes ~300ms wall-clock (vs instant on the
  // main-thread virtual clock). Output is ordered by sleep duration.
  const code = `(async/all
  (list (async (async/sleep 300) (println "c"))
        (async (async/sleep 100) (println "a"))
        (async (async/sleep 200) (println "b"))))
(println "done")`;
  await setEditorCode(page, code);

  // Main-thread responsiveness probe: a timer that must keep ticking while the
  // worker is busy/blocked (it would be frozen on the old main-thread path).
  await page.evaluate(() => {
    (window as any).__ticks = 0;
    (window as any).__t = setInterval(() => { (window as any).__ticks++; }, 20);
  });

  const t0 = Date.now();
  await page.getByTestId('run-btn').click();
  await page.waitForSelector('#output .output-timing', { timeout: 20000 });
  const elapsed = Date.now() - t0;
  const ticks = await page.evaluate(() => {
    clearInterval((window as any).__t);
    return (window as any).__ticks as number;
  });

  // Real wall-clock sleep happened (longest path = 300ms), not instant.
  expect(elapsed).toBeGreaterThan(280);
  // Main thread stayed responsive during the worker's real sleep.
  expect(ticks).toBeGreaterThan(3);
  // Output is correct and ordered by sleep duration, then the final print.
  const lines = await page.$$eval('#output .output-line', (els) => els.map((e) => e.textContent));
  expect(lines).toEqual(['a', 'b', 'c', 'done']);
});

test('worker path: a file written during eval shows up in the file tree (VFS mirror)', async ({ page }) => {
  // Eval runs on the worker (its own VFS); the main-thread interp is a mirror
  // synced via dumpVfs/loadVfs after each run, so the file tree must reflect
  // files the worker created.
  await page.goto('/?worker');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 20000 });

  await setEditorCode(page, '(file/write "/from-worker.txt" "hi from the worker")\n(println "wrote it")');
  await page.getByTestId('run-btn').click();
  await page.waitForSelector('#output .output-timing', { timeout: 20000 });

  // The file the worker created must appear in the (mirror-backed) file tree.
  await page.waitForSelector('.vfs-tree-file:has-text("from-worker.txt")', { timeout: 5000 });
  const files = await page.$$eval('.vfs-tree-file', (els) =>
    els.map((e) => (e.textContent || '').trim())
  );
  expect(files.some((f) => f.includes('from-worker.txt'))).toBe(true);
});

test('worker path: http/get works via synchronous XHR (no replay)', async ({ page }) => {
  // On the worker, http uses a blocking synchronous XHR instead of the
  // main-thread replay-the-whole-program hack. Hit a same-origin file so the
  // test is reliable (no network, no cross-origin CORP/COEP concerns).
  await page.goto('/?worker');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 20000 });

  const origin = await page.evaluate(() => location.origin);
  await setEditorCode(page, `(:status (http/get "${origin}/index.html"))`);
  await page.getByTestId('run-btn').click();
  await page.waitForSelector('#output .output-timing', { timeout: 20000 });

  const errors = await page.$$eval('#output .output-error', (els) => els.map((e) => e.textContent || ''));
  expect(errors.join('\n')).not.toContain('window');
  const value = await page.$eval('#output .output-value', (el) => el.textContent || '');
  expect(value).toContain('200');
});

test('evaluates a recursive fib correctly', async ({ page }) => {
  const code = `(define (fib n)
  (define (go a b i)
    (if (= i 0) a (go b (+ a b) (- i 1))))
  (go 0 1 n))
(fib 20)`;

  await setEditorCode(page, code);
  await clickRunAndWait(page);
  const value = await page.$eval('#output .output-value', el => el.textContent);
  expect(value).toContain('6765');
});

test('hello.sema runs', async ({ page }) => {
  await page.click('.tree-file:text("hello.sema")');
  await clickRunAndWait(page);

  const errorEl = await page.$('#output .output-error');
  expect(errorEl).toBeNull();

  const value = await page.$eval('#output .output-value', el => el.textContent);
  expect(value).toContain('Hello, world!');
});

test('clear button clears output', async ({ page }) => {
  await setEditorCode(page, '(println "test")');
  await clickRunAndWait(page);

  // Output should have content
  const before = await page.getByTestId('output').innerHTML();
  expect(before.length).toBeGreaterThan(0);

  // Clear
  await page.getByTestId('clear-btn').click();
  const after = await page.getByTestId('output').innerHTML();
  expect(after).toBe('');
});
