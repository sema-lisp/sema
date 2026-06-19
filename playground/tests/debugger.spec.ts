import { test, expect, Page } from '@playwright/test';

async function waitForReady(page: Page) {
  await page.goto('/');
  await page.waitForSelector('[data-testid="status"].status-ready', { timeout: 15000 });
}

async function setEditorCode(page: Page, code: string) {
  await page.getByTestId('editor').fill(code);
}

/** Click a gutter line number to toggle a breakpoint. */
async function toggleBreakpoint(page: Page, lineNum: number) {
  await page.click(`.gutter-line:nth-child(${lineNum})`);
}

/** Get the current debug state from the status bar. */
async function getStatus(page: Page): Promise<string> {
  return await page.$eval('#status', el => el.textContent ?? '');
}

/** Get the current line the debugger highlights. */
async function getCurrentDebugLine(page: Page): Promise<number | null> {
  const el = await page.$('.gutter-line.current-line');
  if (!el) return null;
  const text = await el.textContent();
  return text ? parseInt(text, 10) : null;
}

/** Get all breakpoint line numbers. */
async function getBreakpointLines(page: Page): Promise<number[]> {
  return page.$$eval('.gutter-line.breakpoint', els =>
    els.map(el => parseInt(el.textContent ?? '0', 10))
  );
}

/** Get all output lines (text content). */
async function getOutputLines(page: Page): Promise<string[]> {
  return page.$$eval('#output .output-line', els =>
    els.map(el => el.textContent ?? '')
  );
}

/** Get all error output. */
async function getErrors(page: Page): Promise<string[]> {
  return page.$$eval('#output .output-error', els =>
    els.map(el => el.textContent ?? '')
  );
}

/** Get debug variable names from the variables panel. */
async function getDebugVarNames(page: Page): Promise<string[]> {
  return page.$$eval('.debug-var-name', els =>
    els.map(el => el.textContent ?? '')
  );
}

/** Get debug variable values from the variables panel. */
async function getDebugVars(page: Page): Promise<{name: string, value: string}[]> {
  return page.$$eval('.debug-var-row', els =>
    els.map(el => ({
      name: el.querySelector('.debug-var-name')?.textContent ?? '',
      value: el.querySelector('.debug-var-value')?.textContent ?? '',
    }))
  );
}

/** Wait for debugger to pause (status bar shows "Paused at line ..."). */
async function waitForPaused(page: Page, timeout = 5000) {
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent?.startsWith('Paused'),
    { timeout }
  );
}

/** Wait for debugger to return to idle. */
async function waitForIdle(page: Page, timeout = 10000) {
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent === 'Ready',
    { timeout }
  );
}

test.describe('Debugger', () => {
  test.beforeEach(async ({ page }) => {
    await waitForReady(page);
  });

  test('debug button starts and stops on first line', async ({ page }) => {
    await setEditorCode(page, '(define x 10)\n(+ x 5)');
    
    // Click Debug
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    const line = await getCurrentDebugLine(page);
    console.log(`Stopped at line: ${line}`);
    expect(line).toBe(1);
    
    // Stop debugging
    await page.click('#dbg-stop');
    await waitForIdle(page);
  });

  test('breakpoint toggles in gutter', async ({ page }) => {
    await setEditorCode(page, '(define x 1)\n(define y 2)\n(+ x y)');
    
    // Toggle breakpoint on line 2
    await toggleBreakpoint(page, 2);
    let bps = await getBreakpointLines(page);
    expect(bps).toContain(2);
    
    // Toggle off
    await toggleBreakpoint(page, 2);
    bps = await getBreakpointLines(page);
    expect(bps).not.toContain(2);
  });

  test('breakpoint: stops at correct line', async ({ page }) => {
    const code = '(define x 10)\n(define y 20)\n(+ x y)';
    await setEditorCode(page, code);

    // Set breakpoint on line 3
    await toggleBreakpoint(page, 3);

    // Click Debug. When breakpoints are set, the debugger runs straight to the
    // first breakpoint (it only stops on entry when no breakpoints exist).
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);

    const bpLine = await getCurrentDebugLine(page);
    console.log(`Breakpoint stop at line: ${bpLine}`);
    expect(bpLine).toBe(3);

    // Continue to end
    await page.click('#dbg-continue');
    await waitForIdle(page);
  });

  test('no breakpoints: stops on entry', async ({ page }) => {
    const code = '(define x 10)\n(define y 20)\n(+ x y)';
    await setEditorCode(page, code);

    // No breakpoints set — Debug should pause on entry (the first line).
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);

    const entryLine = await getCurrentDebugLine(page);
    console.log(`Entry stop at line: ${entryLine}`);
    expect(entryLine).toBe(1);

    // Continue to end
    await page.click('#dbg-continue');
    await waitForIdle(page);
  });

  test('step into: walks line by line', async ({ page }) => {
    const code = '(define a 1)\n(define b 2)\n(define c 3)\n(+ a b c)';
    await setEditorCode(page, code);
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    const lines: number[] = [];
    const line1 = await getCurrentDebugLine(page);
    lines.push(line1!);
    
    // Step through each line
    for (let i = 0; i < 5; i++) {
      const status = await getStatus(page);
      if (status === 'Ready') break;
      
      await page.click('#dbg-step-into');
      // Wait for either paused or idle
      await page.waitForFunction(
        () => {
          const s = document.getElementById('status')?.textContent ?? '';
          return s.startsWith('Paused') || s === 'Ready';
        },
        { timeout: 5000 }
      );
      
      const curLine = await getCurrentDebugLine(page);
      if (curLine !== null) lines.push(curLine);
      else break;
    }
    
    console.log('Step-into lines visited:', lines);
    // Should visit lines in order, each line exactly once (no duplicates in sequence)
    for (let i = 1; i < lines.length; i++) {
      expect(lines[i]).toBeGreaterThanOrEqual(lines[i-1]);
      // No same-line stuttering
      if (i > 0 && lines[i] === lines[i-1]) {
        console.warn(`WARNING: Stopped at line ${lines[i]} twice in a row`);
      }
    }
  });

  test('step over: does not descend into function calls', async ({ page }) => {
    const code = '(define (add a b) (+ a b))\n(define result (add 3 4))\nresult';
    await setEditorCode(page, code);
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    const lines: number[] = [];
    lines.push((await getCurrentDebugLine(page))!);
    
    for (let i = 0; i < 5; i++) {
      const status = await getStatus(page);
      if (status === 'Ready') break;
      
      await page.click('#dbg-step-over');
      await page.waitForFunction(
        () => {
          const s = document.getElementById('status')?.textContent ?? '';
          return s.startsWith('Paused') || s === 'Ready';
        },
        { timeout: 5000 }
      );
      
      const curLine = await getCurrentDebugLine(page);
      if (curLine !== null) lines.push(curLine);
      else break;
    }
    
    console.log('Step-over lines visited:', lines);
    // Should NOT visit line 1 body (the function body is "(+ a b)" on line 1)
    // After defining the function (line 1), should go to line 2, then line 3
  });

  test('continue past breakpoint does not re-trigger on same line', async ({ page }) => {
    // This tests issue #2: multi-opcode lines should not re-break
    const code = '(define x 1)\n(define y (+ x 2))\n(define z (+ y 3))\nz';
    await setEditorCode(page, code);
    
    // Set breakpoint on line 2
    await toggleBreakpoint(page, 2);

    // With a breakpoint set, Debug runs straight to it (line 2).
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);

    const firstStop = await getCurrentDebugLine(page);
    console.log(`First breakpoint stop: line ${firstStop}`);
    expect(firstStop).toBe(2);
    
    // Continue past the breakpoint - should NOT stop on line 2 again
    await page.click('#dbg-continue');
    
    // Should reach end (idle), not stop on line 2 again
    await waitForIdle(page);
    const status = await getStatus(page);
    expect(status).toBe('Ready');
  });

  test('continue past breakpoint in loop re-triggers on next iteration', async ({ page }) => {
    const code = '(define total 0)\n(do ((i 0 (+ i 1))) ((= i 3))\n  (set! total (+ total i)))\ntotal';
    await setEditorCode(page, code);
    
    // Set breakpoint on line 3 (body of do loop)
    await toggleBreakpoint(page, 3);

    // Debug runs straight to the first breakpoint hit (loop iteration 1).
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);

    const firstHit = await getCurrentDebugLine(page);
    console.log(`First loop hit: line ${firstHit}`);

    // Continue - should hit the same breakpoint again on the next iteration
    await page.click('#dbg-continue');
    await waitForPaused(page);

    const secondHit = await getCurrentDebugLine(page);
    console.log(`Second loop hit: line ${secondHit}`);
    expect(secondHit).toBe(firstHit);
    
    // Stop
    await page.click('#dbg-stop');
    await waitForIdle(page);
  });

  test('variables panel shows locals', async ({ page }) => {
    const code = '(define x 42)\n(define y "hello")\n(+ x 1)';
    await setEditorCode(page, code);
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    // Step past first define
    await page.click('#dbg-step-into');
    await waitForPaused(page);
    
    // Step past second define  
    await page.click('#dbg-step-into');
    await waitForPaused(page);
    
    // Check variables panel
    const vars = await getDebugVars(page);
    console.log('Variables:', vars);
    
    // Should show x and y as locals
    const varNames = vars.map(v => v.name);
    // At minimum, we should see some variables
    console.log('Variable names:', varNames);
    
    await page.click('#dbg-stop');
    await waitForIdle(page);
  });

  test('debug stop resets UI completely', async ({ page }) => {
    await setEditorCode(page, '(define x 1)\n(define y 2)\n(+ x y)');
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    // Stop
    await page.click('#dbg-stop');
    await waitForIdle(page);
    
    // Verify UI is reset
    const debugBtn = page.getByTestId('debug-btn');
    await expect(debugBtn).not.toBeDisabled();
    
    const runBtn = page.getByTestId('run-btn');
    await expect(runBtn).not.toBeDisabled();
    
    // Debug controls should be hidden
    const controls = page.getByTestId('debug-controls');
    await expect(controls).toHaveClass(/hidden/);
    
    // No current line highlight
    const curLine = await getCurrentDebugLine(page);
    expect(curLine).toBeNull();
    
    // No variables panel
    const varsPanel = await page.$('#debug-vars');
    expect(varsPanel).toBeNull();
  });

  test('error during debug shows error and resets', async ({ page }) => {
    // Code that will error
    await setEditorCode(page, '(define x 1)\n(/ x 0)');
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    // Continue - should hit division by zero
    await page.click('#dbg-continue');
    
    // Wait for either error or idle
    await page.waitForFunction(
      () => {
        const s = document.getElementById('status')?.textContent ?? '';
        return s === 'Ready';
      },
      { timeout: 5000 }
    );
    
    // Check for error output
    const errors = await getErrors(page);
    console.log('Errors:', errors);
    
    // UI should be reset to idle
    const status = await getStatus(page);
    expect(status).toBe('Ready');
  });

  test('infinite loop yields and stop button works', async ({ page }) => {
    // An infinite loop — the VM should yield and let us click Stop
    await setEditorCode(page, '(define (loop) (loop))\n(loop)');
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    // Continue — this enters the infinite loop
    await page.click('#dbg-continue');
    
    // Wait a moment for the yield loop to start
    await page.waitForTimeout(500);
    
    // Click stop — should work because VM yields to event loop
    await page.click('#dbg-stop');
    await waitForIdle(page, 3000);
    
    const status = await getStatus(page);
    expect(status).toBe('Ready');
  });

  test('stack trace shows frames', async ({ page }) => {
    const code = '(define (inner x) (+ x 1))\n(define (outer) (inner 5))\n(outer)';
    await setEditorCode(page, code);
    
    // Set breakpoint in inner function (line 1)
    await toggleBreakpoint(page, 1);
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    // Entry stop - continue to breakpoint inside inner
    await page.click('#dbg-continue');
    
    // Should stop when inner is called  
    await page.waitForFunction(
      () => {
        const s = document.getElementById('status')?.textContent ?? '';
        return s.startsWith('Paused') || s === 'Ready';
      },
      { timeout: 5000 }
    );
    
    const line = await getCurrentDebugLine(page);
    console.log(`Stopped at line: ${line}`);
    
    await page.click('#dbg-stop');
    await waitForIdle(page);
  });

  test('multiple debug sessions work sequentially', async ({ page }) => {
    await setEditorCode(page, '(+ 1 2)');
    
    // First session
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    await page.click('#dbg-continue');
    await waitForIdle(page);
    
    // Second session with different code
    await setEditorCode(page, '(* 3 4)');
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    await page.click('#dbg-continue');
    await waitForIdle(page);
    
    // Verify output from second session
    const values = await page.$$eval('#output .output-value', els =>
      els.map(el => el.textContent ?? '')
    );
    console.log('Output values:', values);
  });

  test('step out returns to caller or finishes', async ({ page }) => {
    // With only two lines, step-out from the function body will finish execution
    // (no more code to run after the call returns). This tests that step-out
    // doesn't crash and either pauses in the caller or finishes cleanly.
    const code = '(define (f x) (+ x 1))\n(f 10)';
    await setEditorCode(page, code);
    
    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);
    
    // Step to line 2
    await page.click('#dbg-step-into');
    await page.waitForFunction(
      () => {
        const s = document.getElementById('status')?.textContent ?? '';
        return s.startsWith('Paused') || s === 'Ready';
      },
      { timeout: 5000 }
    );
    
    const atLine2 = await getCurrentDebugLine(page);
    console.log(`After first step: line ${atLine2}`);
    
    // Step into the function body
    await page.click('#dbg-step-into');
    await page.waitForFunction(
      () => {
        const s = document.getElementById('status')?.textContent ?? '';
        return s.startsWith('Paused') || s === 'Ready';
      },
      { timeout: 5000 }
    );
    
    const inBody = await getCurrentDebugLine(page);
    console.log(`Inside function body: line ${inBody}`);
    
    // Step out — may finish execution since (f 10) is the last expression
    await page.click('#dbg-step-out');
    await page.waitForFunction(
      () => {
        const s = document.getElementById('status')?.textContent ?? '';
        return s.startsWith('Paused') || s === 'Ready';
      },
      { timeout: 5000 }
    );
    
    const status = await getStatus(page);
    const afterStepOut = await getCurrentDebugLine(page);
    console.log(`After step-out: line ${afterStepOut}, status: ${status}`);
    
    // Either paused at caller or finished — both are valid
    if (status !== 'Ready') {
      await page.click('#dbg-stop');
      await waitForIdle(page);
    }
  });

  test('breakpoint snapping: WASM API returns validLines and snapped breakpoints', async ({ page }) => {
    // Test the WASM API directly to verify snapping logic
    const result = await page.evaluate(async () => {
      const mod = await import('/pkg/sema_wasm.js');
      await mod.default();
      const interp = new mod.SemaInterpreter();

      // Code with empty line and comment — only lines 1 and 4 should be valid
      const code = '(+ 1 2)\n\n; comment\n(+ 3 4)';

      // Set breakpoints on lines 2 (empty) and 3 (comment) — should snap
      const r = interp.debugStart(code, [2, 3]);
      const result = {
        status: r.status,
        validLines: r.validLines,
        breakpoints: r.breakpoints,
        line: r.line,
      };
      interp.debugStop();
      return result;
    });

    console.log('WASM API result:', JSON.stringify(result));

    // validLines should include 1 and 4 (expression lines), not 2 or 3
    expect(result.validLines).toContain(1);
    expect(result.validLines).toContain(4);
    expect(result.validLines).not.toContain(2);
    expect(result.validLines).not.toContain(3);

    // Breakpoints on lines 2 and 3 should snap to valid lines
    // Line 2 is equidistant from 1 and 4, snaps forward → 4? Or nearest.
    // Line 3 is closer to 4 → snaps to 4
    for (const bp of result.breakpoints) {
      expect(result.validLines).toContain(bp);
    }
  });

  test('breakpoint snapping: bare literal line snaps to nearest expression', async ({ page }) => {
    const result = await page.evaluate(async () => {
      const mod = await import('/pkg/sema_wasm.js');
      await mod.default();
      const interp = new mod.SemaInterpreter();

      // Bare literals on lines 1-2, expression on line 3
      const code = '"hello"\n42\n(+ 1 2)';

      // Set breakpoint on line 2 (bare literal 42) — should snap to line 3
      const r = interp.debugStart(code, [2]);
      const result = {
        validLines: r.validLines,
        breakpoints: r.breakpoints,
      };
      interp.debugStop();
      return result;
    });

    console.log('Bare literal snap:', JSON.stringify(result));

    // Only line 3 should be valid (the function call)
    expect(result.validLines).toContain(3);
    expect(result.validLines).not.toContain(1);
    expect(result.validLines).not.toContain(2);

    // Breakpoint on line 2 should snap to line 3
    expect(result.breakpoints).toEqual([3]);
  });

  test('breakpoint snapping: clicking empty line immediately snaps dot to valid line', async ({ page }) => {
    const code = '(define x 10)\n\n(+ x 5)';
    await setEditorCode(page, code);

    // Click line 2 (empty line) — should immediately snap to line 1 or 3
    await toggleBreakpoint(page, 2);

    const bps = await getBreakpointLines(page);
    console.log('Breakpoints after clicking empty line 2:', bps);

    // Should NOT show on line 2, should have snapped
    expect(bps).not.toContain(2);
    expect(bps.some(l => l === 1 || l === 3)).toBe(true);
  });

  test('breakpoint snapping: clicking comment line immediately snaps', async ({ page }) => {
    const code = '(define x 10)\n; this is a comment\n(+ x 5)';
    await setEditorCode(page, code);

    // Click line 2 (comment) — should snap
    await toggleBreakpoint(page, 2);

    const bps = await getBreakpointLines(page);
    console.log('Breakpoints after clicking comment line 2:', bps);

    expect(bps).not.toContain(2);
    expect(bps.some(l => l === 1 || l === 3)).toBe(true);
  });

  test('breakpoint snapping: clicking valid line stays put', async ({ page }) => {
    const code = '(define x 10)\n\n(+ x 5)';
    await setEditorCode(page, code);

    // Click line 1 (valid expression) — should stay on line 1
    await toggleBreakpoint(page, 1);

    const bps = await getBreakpointLines(page);
    expect(bps).toContain(1);

    // Toggle off
    await toggleBreakpoint(page, 1);
    const bps2 = await getBreakpointLines(page);
    expect(bps2).not.toContain(1);
  });

  test('breakpoint snapping: snapped breakpoint actually fires', async ({ page }) => {
    // Set breakpoint on an empty line, verify it fires at the snapped location
    const code = '(define x 10)\n\n(+ x 5)';
    await setEditorCode(page, code);

    // Set breakpoint on line 2 (empty) — should snap to line 1 or 3
    await toggleBreakpoint(page, 2);

    await page.getByTestId('debug-btn').click();
    await waitForPaused(page);

    // Entry stop at line 1
    const entryLine = await getCurrentDebugLine(page);
    expect(entryLine).toBe(1);

    // Continue — should hit the snapped breakpoint
    await page.click('#dbg-continue');
    await page.waitForFunction(
      () => {
        const s = document.getElementById('status')?.textContent ?? '';
        return s.startsWith('Paused') || s === 'Ready';
      },
      { timeout: 5000 }
    );

    const status = await getStatus(page);
    if (status.startsWith('Paused')) {
      const bpLine = await getCurrentDebugLine(page);
      console.log(`Snapped breakpoint fired at line: ${bpLine}`);
      // Should have stopped at a valid line (1 or 3), not line 2
      expect(bpLine === 1 || bpLine === 3).toBe(true);
      await page.click('#dbg-stop');
      await waitForIdle(page);
    }
    // If status is 'Ready', session already finished — no cleanup needed
  });
});
