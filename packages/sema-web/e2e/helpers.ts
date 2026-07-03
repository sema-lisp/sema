import type { Page } from "@playwright/test";

/** Wait for Sema to finish initializing. Throws if init failed. */
export async function waitForSema(page: Page): Promise<void> {
  await page.waitForFunction(
    () => (window as any).__semaInitialized === true,
    null,
    { timeout: 15000 }
  );

  const initError = await page.evaluate(() => (window as any).__semaInitError);
  if (initError) {
    throw new Error(`SemaWeb.init() failed: ${initError}`);
  }
}

/** Evaluate Sema code via the page's SemaWeb instance. */
export async function semaEval(page: Page, code: string): Promise<any> {
  return page.evaluate(
    (code) => (window as any).__semaWeb.eval(code),
    code
  );
}

/** Capture console.error calls. */
export function captureConsoleErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") errors.push(msg.text());
  });
  return errors;
}

/** Capture browser failure channels that should not fire during recoverable app errors. */
export function captureBrowserFailures(page: Page): { consoleErrors: string[]; pageErrors: string[] } {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") consoleErrors.push(msg.text());
  });
  page.on("pageerror", (err) => {
    pageErrors.push(err.message);
  });
  return { consoleErrors, pageErrors };
}
