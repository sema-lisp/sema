import { test, expect, Page } from '@playwright/test';
import { toggleBreakpoint } from './gutter';

// Regression test for the debug HTTP replay-restart loop: `debugStart`
// unconditionally cleared the HTTP replay cache (`clear_http_cache()`) even
// when JS called it to *replay* a same-session run right after
// `debugPerformFetch` had just cached a response (the `http_needed` ->
// `debugPerformFetch` -> `debugStart` restart cycle). That wiped the response
// before the replay could use it, so the replay missed the cache again,
// re-requested `http_needed`, and looped until `app.js`'s
// `MAX_DEBUG_HTTP_RETRIES` (50) tripped "Exceeded maximum HTTP requests
// during debug session". Fixed by arming a one-shot
// `DEBUG_HTTP_REPLAY_ARMED` flag in `debugPerformFetch` that the very next
// `debugStart` consumes to skip the cache clear.
//
// This test mocks the network so it is deterministic and offline: a single
// real fetch to icanhazdadjoke.com should occur across the whole debug
// session (initial run + any replay restarts), never fifty.

async function waitForReady(page: Page) {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });
}

async function waitForPaused(page: Page, timeout = 30000) {
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent?.startsWith('Paused'),
    { timeout }
  );
}

async function getErrors(page: Page): Promise<string[]> {
  return page.getByTestId('output-error').allTextContents();
}

test('debug session with an HTTP call replays from cache instead of looping', async ({ page }) => {
  let requestCount = 0;

  await page.route('https://icanhazdadjoke.com/*', async route => {
    requestCount++;
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ joke: 'Why did the chicken cross the road? To get to the other side.' }),
    });
  });

  await waitForReady(page);

  const code = await page.evaluate(async () => {
    const resp = await fetch('/examples/http/dad-jokes.sema');
    return resp.text();
  });
  await page.getByTestId('editor').fill(code);

  // Breakpoint on line 12: `(if (= (:status resp) 200)` — right after the
  // http/get call returns, inside the debug-http replay flow.
  await toggleBreakpoint(page, 12);

  await page.getByTestId('debug-btn').click();
  await waitForPaused(page);

  // NOTE: `getCurrentDebugLine`/`getBreakpointLines` (tests/gutter.ts) query
  // `[part~="current"]`/`[part~="breakpoint"]`, but the vendored
  // @sema-lang/ui editor marks the current/breakpoint line with CSS classes
  // ("cur"/"bp"), not extra `part` tokens — those helpers are stale against
  // the current component (pre-existing PG-E2E-1 harness drift, unrelated to
  // this fix). The status bar text is the reliable source of the paused
  // line, so assert on that instead.
  const status = await page.getByTestId('status').textContent();
  expect(status).toBe('Paused at line 12');

  expect(requestCount).toBe(1);

  const errors = await getErrors(page);
  expect(errors.join('\n')).not.toContain('Exceeded maximum HTTP requests');
  expect(errors).toEqual([]);

  await page.getByTestId('dbg-stop').click();
});
