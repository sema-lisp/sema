import { test, expect } from '@playwright/test';
import { toggleBreakpoint, getCurrentDebugLine } from './gutter';

test('debug exchange-rates via UI with HTTP fetch', async ({ page }) => {
  const logs: string[] = [];
  page.on('console', msg => logs.push(`[${msg.type()}] ${msg.text()}`));
  page.on('pageerror', err => logs.push(`[PAGE_ERROR] ${err.message}`));

  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });

  // Load exchange-rates example code
  const code = await page.evaluate(async () => {
    const resp = await fetch('/examples/http/exchange-rates.sema');
    return resp.text();
  });
  await page.getByTestId('editor').fill(code);

  // Set breakpoint on line 13 (the println after HTTP data is parsed).
  await toggleBreakpoint(page, 13);

  // Click Debug
  await page.getByTestId('debug-btn').click();

  // Wait for paused — may take time for HTTP fetch + restart cycle
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent?.startsWith('Paused'),
    { timeout: 30000 }
  );

  const status = await page.getByTestId('status').textContent();
  console.log('Status after debug + HTTP:', status);

  // The debugger should have stopped (either entry or breakpoint).
  const lineNum = await getCurrentDebugLine(page);
  console.log('Stopped at line:', lineNum);

  // Continue to breakpoint at line 13
  while (true) {
    const s = (await page.getByTestId('status').textContent()) ?? '';
    if (s === 'Ready') break;
    if (s.includes('13')) break;

    await page.getByTestId('dbg-continue').click();
    await page.waitForFunction(
      () => {
        const s = document.getElementById('status')?.textContent ?? '';
        return s.startsWith('Paused') || s === 'Ready';
      },
      { timeout: 30000 }
    );
  }

  const finalStatus = await page.getByTestId('status').textContent();
  console.log('Final status:', finalStatus);

  // Stop
  await page.getByTestId('dbg-stop').click();
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent === 'Ready',
    { timeout: 5000 }
  );

  console.log('\n=== Browser logs ===');
  for (const log of logs) console.log(log);
});

test('debug exchange-rates.sema with breakpoint at line 18', async ({ page }) => {
  const logs: string[] = [];
  page.on('console', msg => logs.push(`[${msg.type()}] ${msg.text()}`));
  page.on('pageerror', err => logs.push(`[PAGE_ERROR] ${err.message}`));

  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });

  // Read the exchange-rates example code
  const code = await page.evaluate(async () => {
    const resp = await fetch('/examples/http/exchange-rates.sema');
    return resp.text();
  });
  console.log('Code loaded, lines:', code.split('\n').length);

  // Call debugStart directly via WASM to get precise results
  const result = await page.evaluate(async (args) => {
    const { code, bpLine } = args;
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const trace: any[] = [];
    
    // Start with breakpoint at specified line
    let r = interp.debugStart(code, [bpLine]);
    trace.push({ action: 'start', status: r.status, line: r.line, reason: r.reason, error: r.error });
    
    // Step/continue up to 30 times to see what happens
    for (let i = 0; i < 30; i++) {
      if (r.status === 'finished' || r.status === 'error') break;
      
      // Continue to breakpoint
      r = interp.debugContinue();
      
      // Handle yielded
      let yieldCount = 0;
      while (r.status === 'yielded') {
        r = interp.debugPoll();
        yieldCount++;
        if (yieldCount > 1000) {
          trace.push({ action: 'yield_limit', yieldCount });
          interp.debugStop();
          return trace;
        }
      }
      
      trace.push({
        action: 'continue',
        status: r.status,
        line: r.line,
        reason: r.reason,
        error: r.error?.substring(0, 200),
        yieldCount,
      });
      
      if (r.status === 'finished' || r.status === 'error') break;
    }

    interp.debugStop();
    return trace;
  }, { code, bpLine: 18 });

  console.log('\n=== Debug trace with breakpoint at line 18 ===');
  for (const t of result) {
    console.log(`  ${t.action}: status=${t.status} line=${t.line} reason=${t.reason}${t.error ? ' error=' + t.error : ''}${t.yieldCount ? ' yields=' + t.yieldCount : ''}`);
  }

  // Now test step-into to see line-by-line behavior
  const stepTrace = await page.evaluate(async (code) => {
    const mod = await import('/pkg/sema_wasm.js');
    await mod.default();
    const interp = new mod.SemaInterpreter();

    const trace: any[] = [];
    
    let r = interp.debugStart(code, []);
    trace.push({ action: 'start', status: r.status, line: r.line, reason: r.reason });
    
    // Step into up to 50 times
    for (let i = 0; i < 50; i++) {
      if (r.status === 'finished' || r.status === 'error') break;
      
      r = interp.debugStepInto();
      
      // Handle yielded
      while (r.status === 'yielded') {
        r = interp.debugPoll();
      }
      
      trace.push({
        action: 'step',
        status: r.status,
        line: r.line,
        reason: r.reason,
        error: r.error?.substring(0, 200),
      });
      
      if (r.status === 'finished' || r.status === 'error') break;
    }

    interp.debugStop();
    return trace;
  }, code);

  console.log('\n=== Step-into trace (first 50 steps) ===');
  for (const t of stepTrace) {
    console.log(`  ${t.action}: status=${t.status} line=${t.line} reason=${t.reason}${t.error ? ' error=' + t.error : ''}`);
  }

  console.log('\n=== Browser logs ===');
  for (const log of logs) {
    console.log(log);
  }
});
