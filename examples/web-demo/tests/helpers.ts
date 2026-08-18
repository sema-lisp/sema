import { type Page } from "@playwright/test";

/** Wait for SemaWeb to initialize (or fail with a useful error). */
export async function waitForSema(page: Page) {
  await page.waitForFunction(
    () => {
      const w = window as any;
      return w.__semaWeb || w.__semaInitError;
    },
    null,
    { timeout: 15_000 }
  );
  const initError = await page.evaluate(() => (window as any).__semaInitError);
  if (initError) throw new Error(`SemaWeb init failed: ${initError}`);
}
