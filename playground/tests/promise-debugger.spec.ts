import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { expect, test } from '@playwright/test';

const REPO_ROOT = path.resolve(__dirname, '..', '..');

test('promise debugger preserves the fetch frame and executes the request once', async ({ page }) => {
  let requests = 0;
  await page.route('**/debug-promise-once', async (route) => {
    requests += 1;
    await route.fulfill({ status: 200, body: 'ok' });
  });
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the playground server at runtime.
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    const ordinaryOutput: string[] = [];
    interp.setPromiseOutputSink((_root: number, _stream: string, text: string) => {
      ordinaryOutput.push(text);
    });
    const code = [
      '(define (fetch-once)',
      '  (let ((before 41)',
      '        (response (http/get "' + location.origin + '/debug-promise-once")))',
      '    (println "DEBUG-BEFORE-STOP")',
      '    (print-error "DEBUG-ERR-BEFORE-STOP")',
      '    (+ before (:status response))))',
      '(fetch-once)',
    ].join('\n');

    const stopped = await interp.debugStartPromise(code, [6]);
    const locals = interp.debugGetLocalsPromise();
    const stack = interp.debugGetStackTracePromise();
    const finished = await interp.debugContinuePromise();
    return { stopped, locals, stack, finished, ordinaryOutput };
  });

  expect(requests).toBe(1);
  expect(result.stopped.status).toBe('stopped');
  expect(result.stopped.line).toBe(6);
  expect(result.stopped.output).toEqual([
    'DEBUG-BEFORE-STOP',
    '[error] "DEBUG-ERR-BEFORE-STOP"',
  ]);
  expect(result.stopped.outputEvents).toEqual([
    { stream: 'stdout', text: 'DEBUG-BEFORE-STOP' },
    { stream: 'stderr', text: '[error] "DEBUG-ERR-BEFORE-STOP"' },
  ]);
  expect(result.locals).toEqual(
    expect.arrayContaining([expect.objectContaining({ name: 'before', value: '41' })]),
  );
  expect(result.stack).toEqual(
    expect.arrayContaining([expect.objectContaining({ name: 'fetch-once' })]),
  );
  expect(result.finished).toMatchObject({ status: 'finished', value: '241' });
  expect(result.ordinaryOutput).toEqual([]);
});

test('stopping one promise debugger cancels only its colliding interpreter root', async ({ page }) => {
  await page.route('**/debug-promise-isolation', async (route) => {
    await new Promise((resolve) => setTimeout(resolve, 60));
    await route.fulfill({ status: 200, body: 'ok' });
  });
  await page.goto('/');

  const result = await page.evaluate(async () => {
    // @ts-expect-error -- resolved by the playground server at runtime.
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interpA = new mod.SemaInterpreter();
    const interpB = new mod.SemaInterpreter();
    const actionA = interpA.debugStartPromise(
      '(async/sleep 5000)\n(println "A-MUST-NOT-RUN")',
      [2],
    );
    const actionB = interpB.debugStartPromise(
      '(def response (http/get "' + location.origin + '/debug-promise-isolation"))\n' +
        '(println "B-STOPPED")\n(:status response)',
      [2],
    );

    await new Promise((resolve) => setTimeout(resolve, 20));
    const stopAccepted = interpA.debugStopPromise();
    const [cancelledA, stoppedB] = await Promise.all([actionA, actionB]);
    const finishedB = await interpB.debugContinuePromise();
    const restartedA = await interpA.debugStartPromise('(async/sleep 10)\n"A-restarted"', []);
    const restartedFinishedA = await interpA.debugContinuePromise();
    return {
      stopAccepted,
      cancelledA,
      stoppedB,
      finishedB,
      restartedA,
      restartedFinishedA,
      activeA: interpA.debugIsActivePromise(),
      activeB: interpB.debugIsActivePromise(),
    };
  });

  expect(result.stopAccepted).toBe(true);
  expect(result.cancelledA.status).toBe('cancelled');
  expect(result.cancelledA.rootId).toBe(result.stoppedB.rootId);
  expect(result.cancelledA.output).not.toContain('A-MUST-NOT-RUN');
  expect(result.stoppedB).toMatchObject({ status: 'stopped', line: 2 });
  expect(result.finishedB).toMatchObject({ status: 'finished', value: '200' });
  expect(result.restartedA).toMatchObject({ status: 'stopped', line: 1 });
  expect(result.restartedFinishedA).toMatchObject({ status: 'finished', value: '"A-restarted"' });
  expect(result.activeA).toBe(false);
  expect(result.activeB).toBe(false);
});

test('compiled runEntryAsync archive suspends for a timer and one real HTTP request', async ({ page }) => {
  await page.goto('/');
  const origin = new URL(page.url()).origin;
  const fixtureDir = mkdtempSync(path.join(tmpdir(), 'sema-wasm-archive-'));
  const sourcePath = path.join(fixtureDir, 'entry.sema');
  const archivePath = path.join(fixtureDir, 'entry.vfs');
  writeFileSync(
    sourcePath,
    [
      '(println "ARCHIVE-BEFORE")',
      '(async/sleep 30)',
      `(def response (http/get "${origin}/archive-promise-once"))`,
      '(println "ARCHIVE-AFTER")',
      '(:status response)',
    ].join('\n'),
  );
  try {
    execFileSync(
      'cargo',
      ['run', '--quiet', '-p', 'sema-lang', '--', 'build', '--target', 'web', '-o', archivePath, sourcePath],
      { cwd: REPO_ROOT, stdio: 'pipe' },
    );
    const archive = Array.from(readFileSync(archivePath));
    let requests = 0;
    await page.route('**/archive-promise-once', async (route) => {
      requests += 1;
      await route.fulfill({ status: 200, body: 'ok' });
    });

    const result = await page.evaluate(async (bytes) => {
      // @ts-expect-error -- resolved by the playground server at runtime.
      const mod = await import('/pkg/sema_wasm.js');
      await mod.default();
      const interp = new mod.SemaInterpreter();
      const loaded = interp.loadArchive(new Uint8Array(bytes));
      const started = performance.now();
      const run = await interp.runEntryAsync('__main__.semac');
      const persisted = interp.evalGlobal('(:status response)');
      return { loaded, run, persisted, elapsed: performance.now() - started };
    }, archive);

    expect(result.loaded.error).toBeNull();
    expect(result.run).toMatchObject({ value: '200', error: null });
    expect(result.run.output).toEqual(['ARCHIVE-BEFORE', 'ARCHIVE-AFTER']);
    expect(result.persisted).toMatchObject({ value: '200', error: null });
    expect(result.elapsed).toBeGreaterThanOrEqual(20);
    expect(requests).toBe(1);
  } finally {
    rmSync(fixtureDir, { recursive: true, force: true });
  }
});

test('compiled runEntryAsync adopts the deserialized program into the Promise driver', () => {
  const source = readFileSync(path.join(REPO_ROOT, 'crates/sema-wasm/src/lib.rs'), 'utf8');
  const start = source.indexOf('pub async fn run_entry_async');
  const end = source.indexOf('/// Read a file from the virtual filesystem.', start);
  const method = source.slice(start, end);
  expect(method).toContain('submit_compile_result');
  expect(method).not.toContain('execute_compile_result');
});
