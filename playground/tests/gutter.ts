import { Page } from '@playwright/test';

/* Gutter lines live in <sema-editor>'s shadow DOM (@sema-lang/ui) and expose
 * their state via part tokens (`gutter-line`, plus `breakpoint`/`current`) —
 * the component's public hook for styling and tests. These helpers are the
 * single place that contract is encoded; specs must not query gutter
 * internals directly. */

/** Click a gutter line number to toggle a breakpoint. */
export async function toggleBreakpoint(page: Page, lineNum: number) {
  await page.locator(`[part~="gutter-line"]:nth-child(${lineNum})`).click();
}

/** Get the current line the debugger highlights, or null when not paused. */
export async function getCurrentDebugLine(page: Page): Promise<number | null> {
  const locator = page.locator('[part~="gutter-line"][part~="current"]');
  if ((await locator.count()) === 0) return null;
  const text = await locator.textContent();
  return text ? parseInt(text, 10) : null;
}

/** Get all breakpoint line numbers. */
export async function getBreakpointLines(page: Page): Promise<number[]> {
  const texts = await page.locator('[part~="gutter-line"][part~="breakpoint"]').allTextContents();
  return texts.map(t => parseInt(t, 10));
}
