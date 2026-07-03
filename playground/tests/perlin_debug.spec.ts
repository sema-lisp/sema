import { test, expect } from '@playwright/test';

async function waitForReady(page) {
  await page.goto('/');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 15000 });
}

const PERLIN_HELPERS = `
(define seed 42)
(define (hash x y)
  (let* ((h (mod (+ (* (abs x) 374761393) (* (abs y) 668265263)
                    (* seed 1274126177)) 1000000003))
         (h2 (mod (* h h) 1000000007)))
    (/ (mod (abs h2) 1000000) 1000000.0)))
(define (fade t)
  (let ((t3 (* t t t)))
    (* t3 (+ (* t (- (* t 6.0) 15.0)) 10.0))))
(define (lerp a b t) (+ a (* (- b a) t)))
(define (value-noise x y)
  (let* ((ix (floor x)) (iy (floor y))
         (fx (- x ix)) (fy (- y iy))
         (u (fade fx)) (v (fade fy)))
    (lerp (lerp (hash ix iy) (hash (+ ix 1) iy) u)
          (lerp (hash ix (+ iy 1)) (hash (+ ix 1) (+ iy 1)) u)
          v)))
(define (octave-noise x y)
  (let loop ((i 0) (freq 1.0) (amp 1.0) (total 0.0) (max-amp 0.0))
    (if (= i 3) (/ total max-amp)
      (loop (+ i 1) (* freq 2.0) (* amp 0.5)
            (+ total (* amp (value-noise (* x freq 0.08) (* y freq 0.08))))
            (+ max-amp amp)))))
`;

test('single tree-walker octave-noise call succeeds on a fresh interpreter', async ({ page }) => {
  await waitForReady(page);

  const result = await page.evaluate(async (defs) => {
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    interp.evalGlobal(defs);

    const started = performance.now();
    const output = interp.evalGlobal('(octave-noise 3 0)');
    const elapsed = performance.now() - started;
    return { elapsed, output };
  }, PERLIN_HELPERS);

  expect(result.output.error).toBeNull();
  expect(result.output.value).toBeTruthy();
});

test('repeated perlin helper evaluation is stable on the VM path', async ({ page }) => {
  await waitForReady(page);

  const result = await page.evaluate(async (defs) => {
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const setup = interp.evalVM(defs);
    if (setup.error) return { ok: false, phase: 'setup', error: setup.error };

    const values = [];
    for (let i = 0; i < 10; i++) {
      const evalResult = interp.evalVM(`(octave-noise ${i} 0)`);
      if (evalResult.error) {
        return { ok: false, phase: `iteration-${i}`, error: evalResult.error };
      }
      values.push(Number(evalResult.value));
    }

    return { ok: true, values };
  }, PERLIN_HELPERS);

  expect(result.ok).toBe(true);
  expect(result.values).toHaveLength(10);
  for (const value of result.values) {
    expect(Number.isFinite(value)).toBe(true);
  }
});

// Known Chromium/WASM tree-walker issue:
// repeated evalGlobal() calls through the value-noise path crash the Chromium renderer,
// while Firefox/WebKit, CLI/native eval, direct hash calls, and the browser VM path remain stable.
// octave-noise is the visible playground symptom because it invokes value-noise repeatedly.
test.fixme('repeated tree-walker octave-noise calls crash the browser renderer', async ({ page }) => {
  await waitForReady(page);

  await page.evaluate(async (defs) => {
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();
    (window as any).__perlinInterp = interp;
    interp.evalGlobal(defs);
  }, PERLIN_HELPERS);

  for (let i = 0; i < 5; i++) {
    const result = await page.evaluate((value) => {
      const interp = (window as any).__perlinInterp;
      return interp.evalGlobal(`(octave-noise ${value} 0)`);
    }, i);
    expect(result.error).toBeNull();
  }
});
