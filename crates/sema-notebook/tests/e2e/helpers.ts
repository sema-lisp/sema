// Shared helpers for the notebook e2e suite.
//
// These specs share ONE long-lived server (`workers: 1` in playwright.config.ts).
// `ui.spec.ts` is deliberately hermetic and asserts exact cell counts, markdown
// rendering, and "undo is disabled initially" — so every API-driven suite
// (robustness, persistence, concurrency) must return the notebook to the state
// it found it in. `installNotebookRestore` below installs that cleanup.

import { test as pwTest, expect, Page, APIRequestContext } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

/** Wait for the notebook to finish loading cells. */
export async function waitForLoad(page: Page) {
  await page.goto('/', { waitUntil: 'networkidle' });
  // Cells are loaded via JS fetch after page load — wait for them
  await page.getByTestId('cell').first().waitFor({ timeout: 15000 });
}

/** Get all cells on the page. */
export function cells(page: Page) {
  return page.getByTestId('cell');
}

/** Get cells of a specific type. */
export function cellsOfType(page: Page, type: 'code' | 'markdown') {
  // data-cell-type is a real data attribute (not a testid) used to distinguish
  // code/markdown cells; narrow the testid-selected set with it via `.and()`.
  return page.getByTestId('cell').and(page.locator(`[data-cell-type="${type}"]`));
}

/** Create a cell and return its id, optionally inserted right after `after`. */
export async function createCell(
  request: APIRequestContext,
  source: string,
  type: 'code' | 'markdown' = 'code',
  after?: string
): Promise<string> {
  const data: Record<string, unknown> = { type, source };
  if (after !== undefined) data.after = after;
  const res = await request.post('/api/cells', { data });
  expect(res.ok(), `creating a ${type} cell should succeed`).toBeTruthy();
  const { id } = await res.json();
  expect(id, 'a created cell must come back with an id').toBeTruthy();
  return id;
}

/** Evaluate a cell and return the parsed response body. */
export async function evalCell(request: APIRequestContext, id: string) {
  const res = await request.post(`/api/cells/${id}/eval`);
  expect(res.ok(), `evaluating cell ${id} should return 2xx`).toBeTruthy();
  return res.json();
}

/** The notebook as the server currently has it. */
export async function fetchNotebook(request: APIRequestContext) {
  const res = await request.get('/api/notebook');
  expect(res.ok()).toBeTruthy();
  return res.json();
}

/** A status a client error may legitimately use — anything but 5xx/success. */
export function expectClientError(status: number, what: string) {
  expect(
    status >= 400 && status < 500,
    `${what} should be rejected with a 4xx, got ${status}`
  ).toBeTruthy();
}

// playwright.config.ts serves a COPY of the fixture (target/e2e-demo.sema-nb),
// deliberately, because saving mutates it.
export const NOTEBOOK_FILE = resolve(__dirname, '../../../../target/e2e-demo.sema-nb');

/** The notebook as it exists ON DISK, parsed. */
export function readNotebookFile() {
  return JSON.parse(readFileSync(NOTEBOOK_FILE, 'utf8'));
}

/**
 * Install a beforeAll/afterAll pair that returns the notebook to the state it
 * found it in — see the file header for why this matters.
 *
 * `restoreTitle` also snapshots and restores the notebook title (persistence
 * is the only suite that changes it). `save` additionally calls `/api/save`
 * during cleanup, so the on-disk file matches the restored in-memory state
 * (only persistence saves at all; the others never touch disk).
 *
 * Cleanup deletes by DIFFERENCE against the pristine cell-id set rather than
 * replaying a bookkeeping list of created ids: tests here race deletes against
 * evaluations, so a bookkeeping list drifts from reality; asking the server
 * what is actually there cannot.
 */
export function installNotebookRestore(
  test: typeof pwTest,
  opts: { restoreTitle?: boolean; save?: boolean } = {}
) {
  let pristineOrder: string[] = [];
  let pristineTitle = '';

  test.beforeAll(async ({ request }) => {
    const nb = await fetchNotebook(request);
    pristineOrder = nb.cells.map((c: any) => c.id);
    if (opts.restoreTitle) pristineTitle = nb.title;
  });

  test.afterAll(async ({ request }) => {
    const live = (await fetchNotebook(request)).cells.map((c: any) => c.id);
    for (const id of live.filter((id: string) => !pristineOrder.includes(id))) {
      await request.delete(`/api/cells/${id}`).catch(() => {});
    }
    if (opts.restoreTitle) {
      await request.post('/api/title', { data: { title: pristineTitle } }).catch(() => {});
    }
    if (pristineOrder.length > 0) {
      await request.post('/api/cells/reorder', { data: { cell_ids: pristineOrder } }).catch(() => {});
    }
    // Clears outputs AND the undo history that evaluating enabled.
    await request.post('/api/reset').catch(() => {});
    if (opts.save) {
      await request.post('/api/save').catch(() => {});
    }

    const final = await fetchNotebook(request);
    expect(
      final.cells.map((c: any) => c.id),
      'this suite must leave the notebook exactly as it found it'
    ).toEqual(pristineOrder);
    expect(final.can_undo, 'cleanup must also clear the undo history').toBeFalsy();
  });
}
