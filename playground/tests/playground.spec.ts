import { test, expect, type Page, type Browser } from '@playwright/test';
import { examples } from '../src/examples.js';

const EXAMPLE_NAMES = [
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

const EXAMPLES = EXAMPLE_NAMES.map((name) => {
  for (const category of examples) {
    const file = category.files.find((candidate) => candidate.name === name);
    if (file) {
      return {
        category: category.category,
        id: file.id,
        name: file.name,
      };
    }
  }
  throw new Error(`Example "${name}" not found in generated examples list`);
});

/** Wait for the WASM module to be ready. */
async function waitForReady(page: Page) {
  await page.goto('/');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 15000 });
}

async function openExample(page: Page, example: { category: string; id: string; name: string }) {
  const button = page.locator(`[data-example-id="${example.id}"]`);
  const isCollapsed = await button.evaluate((el) =>
    el.parentElement?.classList.contains('collapsed') ?? false
  );

  if (isCollapsed) {
    const header = page.locator('.tree-category', { hasText: example.category }).first();
    await header.click();
  }

  await button.click();
}

function isAffectedChromiumPerlin(browserName: string, browser: Browser, exampleName: string) {
  if (exampleName !== 'perlin-noise.sema') return false;
  if (browserName !== 'chromium') return false;
  if (process.arch !== 'arm64') return false;

  const major = Number.parseInt(browser.version().split('.')[0] ?? '', 10);
  return Number.isFinite(major) && major < 147;
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

// ── Example smoke tests ──

for (const example of EXAMPLES) {
  test(`example: ${example.name}`, async ({ page, browser, browserName }) => {
    test.fixme(
      isAffectedChromiumPerlin(browserName, browser, example.name),
      'Chromium <147 on ARM64 crashes in V8 when the tree-walker evaluates the perlin value-noise path'
    );

    await openExample(page, example);

    // Verify editor has content
    const editorValue = await page.getByTestId('editor').inputValue();
    expect(editorValue.length).toBeGreaterThan(10);

    // Click Run
    await clickRunAndWait(page);

    // Check there's no error
    const errorEl = await page.$('#output .output-error');
    if (errorEl) {
      const errorText = await errorEl.textContent();
      throw new Error(`Example "${example.name}" produced error: ${errorText}`);
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
  const maze = EXAMPLES.find((example) => example.name === 'maze.sema');
  if (!maze) {
    throw new Error('Maze example not found in generated examples list');
  }

  await openExample(page, maze);
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

test('?no-worker forces the main-thread fallback (instant virtual-clock sleep)', async ({ page }) => {
  // The worker path is the default under cross-origin isolation; ?no-worker
  // opts out to the main-thread interpreter, where async/sleep is an instant
  // no-op. A 2s sleep must therefore complete near-instantly (proving fallback).
  await page.goto('/?no-worker');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 20000 });

  await setEditorCode(page, '(await (async (async/sleep 2000) 42))');
  const t0 = Date.now();
  await page.getByTestId('run-btn').click();
  await page.waitForSelector('#output .output-timing', { timeout: 20000 });
  const elapsed = Date.now() - t0;

  expect(elapsed).toBeLessThan(1000); // instant on the main-thread path (not real-slept)
  const value = await page.$eval('#output .output-value', (el) => el.textContent || '');
  expect(value).toContain('42');
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

test('worker path: upload a file and read it from a script (VFS upload + mirror)', async ({ page }) => {
  await page.goto('/?worker');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 20000 });

  // Upload a file via the hidden file input — it lands in the VFS at /uploads/.
  await page.setInputFiles('#vfs-upload', {
    name: 'notes.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from('hello from an uploaded file'),
  });

  // It shows up in the file tree.
  await page.waitForSelector('.vfs-tree-file:has-text("notes.txt")', { timeout: 5000 });

  // A worker-run script can read it — the uploaded file is seeded into the
  // worker via the VFS mirror (dumpVfs/loadVfs).
  await setEditorCode(page, '(file/read "/uploads/notes.txt")');
  await page.getByTestId('run-btn').click();
  await page.waitForFunction(
    () => {
      const s = document.getElementById('status')?.textContent || '';
      return s === 'Ready' || s === 'Error';
    },
    { timeout: 20000 }
  );
  const value = await page.$eval('#output .output-value', (el) => el.textContent || '');
  expect(value).toContain('hello from an uploaded file');
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

test('worker path: output streams live (incrementally), not all at the end', async ({ page }) => {
  await page.goto('/?worker');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 20000 });

  // Three prints separated by real ~300ms sleeps. With live streaming the lines
  // must appear one at a time as the program runs — not batched at the end.
  await setEditorCode(
    page,
    '(let loop ((i 1)) (if (> i 3) (println "fin") (begin (await (async (async/sleep 300))) (println (str "nap " i)) (loop (+ i 1)))))'
  );
  await page.getByTestId('run-btn').click();

  // While still running, at least one line should already be visible.
  await page.waitForFunction(
    () => document.querySelectorAll('#output .output-line').length >= 1,
    { timeout: 2000 }
  );
  const midRun = await page.$$eval('#output .output-line', (els) => els.length);
  // Not all 4 lines yet (the later naps haven't elapsed) — proves it's live.
  expect(midRun).toBeLessThan(4);

  await page.waitForFunction(
    () => {
      const s = document.getElementById('status')?.textContent || '';
      return s === 'Ready' || s === 'Error';
    },
    { timeout: 5000 }
  );
  const lines = await page.$$eval('#output .output-line', (els) => els.map((e) => e.textContent));
  expect(lines).toEqual(['nap 1', 'nap 2', 'nap 3', 'fin']);
});

test('worker path: Stop cancels a running program and the worker survives', async ({ page }) => {
  await page.goto('/?worker');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 20000 });

  // A long real sleep (5s) via the scheduler (async/await), so on the worker it
  // actually blocks ~5s on Atomics.wait — giving us a window to cancel it.
  await setEditorCode(page, '(await (async (async/sleep 5000) (println "should not print")))');
  await page.getByTestId('run-btn').click();

  // The Run button becomes "Stop" while running.
  await page.waitForFunction(
    () => document.getElementById('run-btn')?.textContent?.includes('Stop'),
    { timeout: 5000 }
  );

  // Click Stop; it must cancel well under the 5s sleep.
  const t0 = Date.now();
  await page.getByTestId('run-btn').click();
  await page.waitForSelector('#output .output-timing', { timeout: 4000 });
  const elapsed = Date.now() - t0;
  expect(elapsed).toBeLessThan(3000); // cancelled, not waited out

  await page.waitForFunction(
    () => document.getElementById('status')?.textContent === 'Stopped',
    { timeout: 4000 }
  );
  const lines = await page.$$eval('#output .output-line', (els) => els.map((e) => e.textContent || ''));
  expect(lines).not.toContain('should not print');

  // The worker survived: a subsequent run works. Wait on the status settling
  // (the prior run's output/timing is only cleared once this run renders).
  await setEditorCode(page, '(+ 20 22)');
  await page.getByTestId('run-btn').click();
  await page.waitForFunction(
    () => {
      const s = document.getElementById('status')?.textContent || '';
      return s === 'Ready' || s === 'Error';
    },
    { timeout: 20000 }
  );
  const value = await page.$eval('#output .output-value', (el) => el.textContent || '');
  expect(value).toContain('42');
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
