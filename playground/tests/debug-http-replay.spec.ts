import { test, expect, Page } from '@playwright/test';
import { getCurrentDebugLine, toggleBreakpoint } from './gutter';

// Regression test for the promise-driven debugger's single-execution HTTP
// contract. The paused VM must resume the same frame after fetch; restarting
// the program would duplicate the request and any earlier side effects.

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

async function waitForIdle(page: Page, timeout = 30000) {
  await page.waitForFunction(
    () => document.getElementById('status')?.textContent === 'Ready',
    { timeout }
  );
}

async function getErrors(page: Page): Promise<string[]> {
  return page.getByTestId('output-error').allTextContents();
}

test('debug session resumes one HTTP request without replaying the program', async ({ page }) => {
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

  const code = [
    '(define (fetch-once)',
    '  (let ((before (begin',
    '                  (context/set :debug-http-runs (+ 1 (or (context/get :debug-http-runs) 0)))',
    '                  (context/get :debug-http-runs)))',
    '        (resp (http/get "https://icanhazdadjoke.com/debug-single-execution")))',
    '    (println before)',
    '    (:status resp)))',
    '(fetch-once)',
  ].join('\n');
  await page.getByTestId('editor').fill(code);

  // The context counter runs before HTTP. A replay restart increments it twice;
  // resuming the original suspended root leaves it at one.
  await toggleBreakpoint(page, 7);

  await page.getByTestId('debug-btn').click();
  await waitForPaused(page);

  expect(await getCurrentDebugLine(page)).toBe(7);
  await expect(page.getByTestId('debug-vars')).toBeVisible();
  expect(await page.getByTestId('debug-var-name').allTextContents()).toEqual(
    expect.arrayContaining(['before', 'resp']),
  );

  expect(requestCount).toBe(1);

  const errors = await getErrors(page);
  expect(errors.join('\n')).not.toContain('Exceeded maximum HTTP requests');
  expect(errors).toEqual([]);

  await page.getByTestId('dbg-continue').click();
  await waitForIdle(page);
  expect(await page.getByTestId('output-line').allTextContents()).toContain('1');
  expect(await page.getByTestId('output-line').allTextContents()).not.toContain('2');
});
