import { expect, Page } from '@playwright/test';

/** Wait for the playground to finish loading the WASM runtime and go ready. */
export async function waitForReady(page: Page) {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });
}

/** Replace the editor's contents with the given source code. */
export async function setEditorCode(page: Page, code: string) {
  await page.getByTestId('editor').fill(code);
}

/** Read all error lines currently shown in the output panel. */
export async function getErrors(page: Page): Promise<string[]> {
  return page.getByTestId('output-error').allTextContents();
}

/** Read the current status text (e.g. "Ready", "Running", "Paused at ..."). */
export async function getStatus(page: Page): Promise<string> {
  return await page.getByTestId('status').textContent() ?? '';
}

/** Read all non-error output lines currently shown in the output panel. */
export async function getOutputLines(page: Page): Promise<string[]> {
  return page.getByTestId('output-line').allTextContents();
}
