import { Page } from '@playwright/test';

/* Gutter lines live in <sema-editor>'s shadow DOM (@sema-lang/ui). The public
 * part identifies each line; the component's `cur` and `bp` state classes
 * identify the current line and breakpoints. These helpers are the single
 * place that contract is encoded; specs must not query gutter internals
 * directly. */

/** Click a gutter line number to toggle a breakpoint. */
export async function toggleBreakpoint(page: Page, lineNum: number) {
  await page.locator(`[part~="gutter-line"]:nth-child(${lineNum})`).click();
}

/** Get the current line the debugger highlights, or null when not paused. */
export async function getCurrentDebugLine(page: Page): Promise<number | null> {
  const locator = page.locator('[part~="gutter-line"].cur');
  if ((await locator.count()) === 0) return null;
  const text = await locator.textContent();
  return text ? parseInt(text, 10) : null;
}

/** Get all breakpoint line numbers. */
export async function getBreakpointLines(page: Page): Promise<number[]> {
  const texts = await page.locator('[part~="gutter-line"].bp').allTextContents();
  return texts.map(t => parseInt(t, 10));
}
