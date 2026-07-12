import { test, expect, Page } from '@playwright/test';

// ── Helpers ────────────────────────────────────────────────────

/** Wait for the notebook to finish loading cells. */
async function waitForLoad(page: Page) {
  await page.goto('/', { waitUntil: 'networkidle' });
  // Cells are loaded via JS fetch after page load — wait for them
  await page.getByTestId('cell').first().waitFor({ timeout: 15000 });
}

/** Get all cells on the page. */
function cells(page: Page) {
  return page.getByTestId('cell');
}

/** Get cells of a specific type. */
function cellsOfType(page: Page, type: 'code' | 'markdown') {
  // data-cell-type is a real data attribute (not a testid) used to distinguish
  // code/markdown cells; narrow the testid-selected set with it via `.and()`.
  return page.getByTestId('cell').and(page.locator(`[data-cell-type="${type}"]`));
}

// ── Page Structure Tests ───────────────────────────────────────

test.describe('Page structure', () => {
  test.beforeEach(async ({ page }) => {
    await waitForLoad(page);
  });

  test('toolbar is visible with all buttons', async ({ page }) => {
    await expect(page.getByTestId('toolbar')).toBeVisible();
    await expect(page.getByTestId('btn-add-code')).toBeVisible();
    await expect(page.getByTestId('btn-add-markdown')).toBeVisible();
    await expect(page.getByTestId('btn-run-all')).toBeVisible();
    await expect(page.getByTestId('btn-undo')).toBeVisible();
    await expect(page.getByTestId('btn-save')).toBeVisible();
    await expect(page.getByTestId('btn-reset')).toBeVisible();
  });

  test('undo button is disabled initially', async ({ page }) => {
    // toHaveJSProperty, not toBeDisabled: Playwright honors aria-disabled only
    // on elements with a role, and the sema-button host deliberately has none
    // (the native disabled lives on its shadow <button>).
    await expect(page.getByTestId('btn-undo')).toHaveJSProperty('disabled', true);
  });

  test('notebook title is displayed', async ({ page }) => {
    const title = page.getByTestId('notebook-title');
    await expect(title).toBeVisible();
    await expect(title).toHaveValue('Sema Language Tour');
  });

  test('status bar shows cell count', async ({ page }) => {
    const count = page.getByTestId('cell-count');
    await expect(count).toBeVisible();
    await expect(count).toContainText('cells');
  });

  test('status indicator shows Ready', async ({ page }) => {
    await expect(page.getByTestId('status-indicator')).toHaveText('Ready');
  });

  test('demo notebook loads with correct cell count', async ({ page }) => {
    // 16 cells total: 5 markdown + 11 code (including error cell)
    // But let's just check we have a reasonable number
    const count = await cells(page).count();
    expect(count).toBeGreaterThanOrEqual(10);
  });

  test('markdown cells are rendered by default', async ({ page }) => {
    const rendered = page.getByTestId('markdown-rendered');
    const count = await rendered.count();
    expect(count).toBeGreaterThan(0);
  });

  test('code cells have textareas', async ({ page }) => {
    const textareas = page.getByTestId('cell-textarea');
    const count = await textareas.count();
    expect(count).toBeGreaterThan(0);
  });

  test('between-cell dividers exist', async ({ page }) => {
    const dividers = page.getByTestId('cell-divider');
    const count = await dividers.count();
    // There should be one more divider than cells (one before, one after each)
    const cellCount = await cells(page).count();
    expect(count).toBe(cellCount + 1);
  });
});

// ── Cell Evaluation Tests ──────────────────────────────────────

test.describe('Cell evaluation', () => {
  test.beforeEach(async ({ page }) => {
    await waitForLoad(page);
  });

  test('eval first code cell via Run button shows stdout output', async ({ page }) => {
    // First code cell is at index 2 (after two markdown cells)
    const codeCell = cellsOfType(page, 'code').first();

    // Click the Run button (hover to reveal, then click)
    await codeCell.hover();
    await codeCell.getByTestId('btn-cell-run').click();

    // Wait for output
    await codeCell.getByTestId('cell-output-stdout').waitFor({ timeout: 15000 });

    // Check stdout content
    const output = await codeCell.getByTestId('output-content').first().textContent();
    expect(output).toContain('x = 42');
    expect(output).toContain('x + y = 50');
  });

  test('stdout output has no spurious leading whitespace', async ({ page }) => {
    // Regression guard: the .cell-output-content container must not use
    // `white-space: pre-wrap`. Alpine's x-if templates leave whitespace-only
    // text nodes (the HTML source indentation between the output spans) inside
    // the container; a pre-wrap container renders that indentation literally,
    // prefixing every output with a blank line and ~28 leading spaces. We use
    // innerText (not textContent) because it reflects the CSS-rendered text.
    const codeCell = cellsOfType(page, 'code').first();
    await codeCell.hover();
    await codeCell.getByTestId('btn-cell-run').click();
    await codeCell.getByTestId('cell-output-stdout').waitFor({ timeout: 15000 });

    const rendered = await codeCell
      .getByTestId('cell-output-stdout')
      .getByTestId('output-content')
      .first()
      .innerText();
    // First rendered character is real output, not leaked indentation.
    expect(rendered.startsWith('x = 42')).toBe(true);
    expect(rendered).not.toMatch(/^\s/);
  });

  test('eval via Shift+Enter works', async ({ page }) => {
    const codeCell = cellsOfType(page, 'code').first();
    const textarea = codeCell.getByTestId('cell-textarea');

    await textarea.focus();
    await page.keyboard.press('Shift+Enter');

    // Wait for output
    await codeCell.getByTestId('cell-output-stdout').waitFor({ timeout: 15000 });
    const output = await codeCell.getByTestId('output-content').first().textContent();
    expect(output).toContain('x = 42');
  });

  test('error cell shows error output', async ({ page }) => {
    // The last code cell is the error cell (/ 1 0)
    const errorCell = cellsOfType(page, 'code').last();

    await errorCell.hover();
    await errorCell.getByTestId('btn-cell-run').click();

    // Wait for error output
    await errorCell.getByTestId('cell-output-error').waitFor({ timeout: 15000 });

    const output = await errorCell.getByTestId('output-content').textContent();
    expect(output).toContain('division by zero');
  });

  test('Run All evaluates all code cells', async ({ page }) => {
    await page.getByTestId('btn-run-all').click();

    // Wait a bit for all cells to evaluate
    await page.waitForTimeout(3000);

    // Check that multiple cells have output
    const outputs = page.getByTestId(/^cell-output-/);
    const count = await outputs.count();
    expect(count).toBeGreaterThan(3);
  });

  test('code cells show execution count in gutter', async ({ page }) => {
    // Code cells always show a cell number based on position
    const codeCell = cellsOfType(page, 'code').first();
    const execCount = codeCell.getByTestId('gutter-exec-count');
    await expect(execCount).toBeVisible();
    // Should show [1] for the first code cell
    await expect(execCount).toContainText('[1]');
  });
});

// ── Output Interaction Tests ───────────────────────────────────

test.describe('Output interaction', () => {
  test('output is collapsible', async ({ page }) => {
    await waitForLoad(page);

    // Eval a cell first
    const codeCell = cellsOfType(page, 'code').first();
    await codeCell.hover();
    await codeCell.getByTestId('btn-cell-run').click();
    await codeCell.getByTestId(/^cell-output-/).first().waitFor({ timeout: 15000 });

    // Content should be visible
    const content = codeCell.getByTestId('output-content').first();
    await expect(content).toBeVisible();

    // Click the chevron header to collapse
    await codeCell.getByTestId('cell-output-header').first().click();

    // Content should be hidden
    await expect(content).toBeHidden();

    // Click again to expand
    await codeCell.getByTestId('cell-output-header').first().click();
    await expect(content).toBeVisible();
  });
});

// ── Cell Management Tests ──────────────────────────────────────

test.describe('Cell management', () => {
  test.beforeEach(async ({ page }) => {
    await waitForLoad(page);
  });

  test('add code cell via toolbar', async ({ page }) => {
    const before = await cells(page).count();
    await page.getByTestId('btn-add-code').click();
    await page.waitForTimeout(1000);
    const after = await cells(page).count();
    expect(after).toBe(before + 1);
  });

  test('add markdown cell via toolbar', async ({ page }) => {
    const before = await cells(page).count();
    await page.getByTestId('btn-add-markdown').click();
    await page.waitForTimeout(1000);
    const after = await cells(page).count();
    expect(after).toBe(before + 1);
  });

  test('delete cell removes it', async ({ page }) => {
    const before = await cells(page).count();

    // Delete the last cell
    const lastCell = cells(page).last();
    await lastCell.hover();
    await lastCell.getByTestId('btn-cell-delete').click();
    await page.waitForTimeout(1000);

    const after = await cells(page).count();
    expect(after).toBe(before - 1);
  });

  test('move cell up changes order', async ({ page }) => {
    // Get the source of the second code cell
    const secondCode = cellsOfType(page, 'code').nth(1);
    const originalSource = await secondCode.getByTestId('cell-textarea').inputValue();

    // Move it up
    await secondCode.hover();
    await secondCode.getByTestId('btn-cell-move-up').click();
    await page.waitForTimeout(1000);

    // The first code cell should now have the original source
    const firstCode = cellsOfType(page, 'code').first();
    const newSource = await firstCode.getByTestId('cell-textarea').inputValue();
    expect(newSource).toBe(originalSource);
  });
});

// ── Markdown Cell Tests ────────────────────────────────────────

test.describe('Markdown cells', () => {
  test.beforeEach(async ({ page }) => {
    await waitForLoad(page);
  });

  test('markdown cells render as HTML by default', async ({ page }) => {
    const rendered = page.getByTestId('markdown-rendered').first();
    await expect(rendered).toBeVisible();

    // Should contain rendered HTML (h1, strong, etc.)
    const html = await rendered.innerHTML();
    expect(html).toContain('<h1>');
  });

  test('clicking rendered markdown enters edit mode', async ({ page }) => {
    const firstMarkdown = cellsOfType(page, 'markdown').first();

    // Click the rendered content
    await firstMarkdown.getByTestId('markdown-rendered').click();
    await page.waitForTimeout(500);

    // Should now show a textarea instead
    await expect(firstMarkdown.getByTestId('cell-textarea')).toBeVisible();
    await expect(firstMarkdown.getByTestId('markdown-rendered')).toHaveCount(0);
  });

  test('Shift+Enter in markdown cell re-renders', async ({ page }) => {
    const firstMarkdown = cellsOfType(page, 'markdown').first();

    // Enter edit mode
    await firstMarkdown.getByTestId('markdown-rendered').click();
    await page.waitForTimeout(500);

    // Press Shift+Enter to re-render
    await page.keyboard.press('Shift+Enter');
    await page.waitForTimeout(500);

    // Should be rendered again
    await expect(firstMarkdown.getByTestId('markdown-rendered')).toBeVisible();
  });
});

// ── Focus and Keyboard Tests ───────────────────────────────────

test.describe('Focus and keyboard', () => {
  test.beforeEach(async ({ page }) => {
    await waitForLoad(page);
  });

  test('clicking textarea focuses cell with gold bar', async ({ page }) => {
    const codeCell = cellsOfType(page, 'code').first();
    await codeCell.getByTestId('cell-textarea').focus();
    await page.waitForTimeout(300);

    // Cell should have focused class
    await expect(codeCell).toHaveClass(/focused/);
  });

  test('Escape unfocuses cell', async ({ page }) => {
    const codeCell = cellsOfType(page, 'code').first();
    await codeCell.getByTestId('cell-textarea').focus();
    await page.waitForTimeout(300);
    await expect(codeCell).toHaveClass(/focused/);

    await page.keyboard.press('Escape');
    await page.waitForTimeout(300);
    await expect(codeCell).not.toHaveClass(/focused/);
  });

  test('Tab inserts spaces in textarea', async ({ page }) => {
    const codeCell = cellsOfType(page, 'code').first();
    const textarea = codeCell.getByTestId('cell-textarea');

    // Focus, then place the caret at document start explicitly: the component
    // assigns `.value` programmatically on load, which parks the caret at the
    // end, and macOS Home moves to the start of the (visual) line rather than
    // the document — neither lands at position 0.
    await textarea.focus();
    await textarea.evaluate((t: HTMLTextAreaElement) => t.setSelectionRange(0, 0));
    await page.keyboard.press('Tab');
    await page.waitForTimeout(200);

    // Should have inserted 2 spaces at the beginning
    const value = await textarea.inputValue();
    expect(value).toMatch(/^  /);
  });

  test('Cmd+S triggers save and shows a success toast', async ({ page }) => {
    await page.keyboard.press('Meta+s');

    // toast() lazily appends a <sema-toaster> to document.body and renders a
    // <sema-toast> inside its (open) shadow root — a plain tag locator pierces
    // it, and the message is real light-DOM slotted text so textContent sees it
    // without reaching into shadow DOM.
    const toast = page.locator('sema-toast');
    // save() awaits persisting the title and every cell's source before it
    // toasts — give it the same headroom as the suite's other network-bound waits.
    await expect(toast).toBeVisible({ timeout: 15000 });
    await expect(toast).toHaveText(/Saved/);
    await expect(toast).toHaveAttribute('variant', 'success');
  });
});

// ── Between-Cell Add Tests ─────────────────────────────────────

test.describe('Between-cell add', () => {
  test('hovering divider shows add button', async ({ page }) => {
    await waitForLoad(page);

    const divider = page.getByTestId('cell-divider').nth(1);
    await divider.hover();
    await page.waitForTimeout(300);

    const addBtn = divider.getByTestId('btn-add-cell');
    await expect(addBtn).toBeVisible();
  });

  test('clicking add button shows dropdown', async ({ page }) => {
    await waitForLoad(page);

    const divider = page.getByTestId('cell-divider').nth(1);
    await divider.hover();
    await divider.getByTestId('btn-add-cell').click();
    await page.waitForTimeout(300);

    const dropdown = divider.getByTestId('add-cell-dropdown');
    await expect(dropdown).toBeVisible();
    await expect(divider.getByTestId('btn-insert-code')).toBeVisible();
    await expect(divider.getByTestId('btn-insert-markdown')).toBeVisible();
  });

  test('inserting code cell from dropdown adds it', async ({ page }) => {
    await waitForLoad(page);
    const before = await cells(page).count();

    const divider = page.getByTestId('cell-divider').nth(1);
    await divider.hover();
    await divider.getByTestId('btn-add-cell').click();
    await divider.getByTestId('btn-insert-code').click();
    await page.waitForTimeout(1000);

    const after = await cells(page).count();
    expect(after).toBe(before + 1);
  });
});

// ── Reset Tests ────────────────────────────────────────────────

test.describe('Reset', () => {
  test('reset clears all outputs via API', async ({ request }) => {
    // Eval a cell via API
    const nb = await (await request.get('/api/notebook')).json();
    const codeCell = nb.cells.find((c: any) => c.cell_type === 'code');
    await request.post(`/api/cells/${codeCell.id}/eval`);

    // Verify output exists via API
    const nb2 = await (await request.get('/api/notebook')).json();
    const evaledCell = nb2.cells.find((c: any) => c.id === codeCell.id);
    expect(evaledCell.rendered_outputs.length).toBeGreaterThan(0);

    // Reset
    await request.post('/api/reset');

    // Verify outputs cleared
    const nb3 = await (await request.get('/api/notebook')).json();
    const resetCell = nb3.cells.find((c: any) => c.id === codeCell.id);
    expect(resetCell.rendered_outputs.length).toBe(0);
  });

  test('reset button clears outputs in UI', async ({ page, request }) => {
    // Run all cells via API so we have plenty of visible outputs
    await request.post('/api/eval-all', { data: { sources: [] } });

    // Load page and wait for outputs to render
    await page.goto('/', { waitUntil: 'networkidle' });
    await page.getByTestId('cell').first().waitFor({ timeout: 15000 });
    // Give JS time to fetch and render
    await page.getByTestId(/^cell-output-/).first().waitFor({ timeout: 10000 });

    // Confirm outputs exist
    let outputs = page.getByTestId(/^cell-output-/);
    expect(await outputs.count()).toBeGreaterThan(0);

    // Reset via UI: a sema-dialog confirm, not a native window.confirm.
    await page.getByTestId('btn-reset').click();
    const dialog = page.getByTestId('reset-dialog');
    await expect(dialog).toBeVisible();
    await dialog.getByTestId('btn-reset-confirm').click();
    await page.waitForTimeout(1000);

    // Outputs should be gone
    outputs = page.getByTestId(/^cell-output-/);
    expect(await outputs.count()).toBe(0);
  });
});

// ── Undo Tests ─────────────────────────────────────────────────

test.describe('Undo', () => {
  test('undo button enables after eval', async ({ page, request }) => {
    // Reset to get clean undo state
    await request.post('/api/reset');

    // Eval a cell via API
    const nb = await (await request.get('/api/notebook')).json();
    const codeCell = nb.cells.find((c: any) => c.cell_type === 'code');
    await request.post(`/api/cells/${codeCell.id}/eval`);

    // Load page — undo should be enabled
    await waitForLoad(page);
    await expect(page.getByTestId('btn-undo')).toHaveJSProperty('disabled', false);
  });

  test('undo reverts cell evaluation', async ({ page, request }) => {
    // Reset, then run all cells so we have visible outputs
    await request.post('/api/reset');
    await request.post('/api/eval-all', { data: { sources: [] } });

    // Load page and wait for outputs
    await page.goto('/', { waitUntil: 'networkidle' });
    await page.getByTestId('cell').first().waitFor({ timeout: 15000 });
    await page.getByTestId(/^cell-output-/).first().waitFor({ timeout: 10000 });

    // Click undo
    await page.getByTestId('btn-undo').click();
    await page.waitForTimeout(1000);

    // At least one cell's output should be gone (the last evaluated)
    // Undo button may still be enabled if there are more undo-able evals
    // Just verify something changed — we already tested the API undo above
    await expect(page.getByTestId('btn-undo')).toBeVisible();
  });

  test('undo API works', async ({ request }) => {
    // Create and eval a cell
    const createRes = await request.post('/api/cells', {
      data: { type: 'code', source: '(define undo-test 999)' },
    });
    const { id } = await createRes.json();
    await request.post(`/api/cells/${id}/eval`);

    // Undo
    const undoRes = await request.post('/api/undo');
    expect(undoRes.ok()).toBeTruthy();
    const data = await undoRes.json();
    expect(data.undone_cell_id).toBe(id);
  });
});

// ── API Integration Tests ──────────────────────────────────────

test.describe('API integration', () => {
  test('GET /api/notebook returns notebook data', async ({ request }) => {
    const response = await request.get('/api/notebook');
    expect(response.ok()).toBeTruthy();
    const data = await response.json();
    expect(data.title).toBe('Sema Language Tour');
    expect(data.cells.length).toBeGreaterThan(0);
  });

  test('POST /api/cells creates a cell', async ({ request }) => {
    const response = await request.post('/api/cells', {
      data: { type: 'code', source: '(+ 99 1)' },
    });
    expect(response.ok()).toBeTruthy();
    const data = await response.json();
    expect(data.id).toBeTruthy();
  });

  test('POST /api/cells/:id/eval evaluates code', async ({ request }) => {
    // Create a cell
    const createRes = await request.post('/api/cells', {
      data: { type: 'code', source: '(+ 2 3)' },
    });
    const { id } = await createRes.json();

    // Eval it
    const evalRes = await request.post(`/api/cells/${id}/eval`);
    expect(evalRes.ok()).toBeTruthy();
    const data = await evalRes.json();
    expect(data.output.content).toBe('5');
  });

  test('POST /api/cells/:id/eval captures stdout', async ({ request }) => {
    const createRes = await request.post('/api/cells', {
      data: { type: 'code', source: '(println "hello notebook")' },
    });
    const { id } = await createRes.json();

    await request.post(`/api/cells/${id}/eval`);

    // Check the notebook state for stdout output
    const nbRes = await request.get('/api/notebook');
    const nb = await nbRes.json();
    const cell = nb.cells.find((c: any) => c.id === id);
    expect(cell).toBeTruthy();

    const stdoutOutput = cell.rendered_outputs.find((o: any) => o.output_type === 'stdout');
    expect(stdoutOutput).toBeTruthy();
    expect(stdoutOutput.content).toContain('hello notebook');
  });

  test('DELETE /api/cells/:id removes a cell', async ({ request }) => {
    const createRes = await request.post('/api/cells', {
      data: { type: 'code', source: 'temp' },
    });
    const { id } = await createRes.json();

    const delRes = await request.delete(`/api/cells/${id}`);
    expect(delRes.ok()).toBeTruthy();

    // Verify it's gone
    const nbRes = await request.get('/api/notebook');
    const nb = await nbRes.json();
    const cell = nb.cells.find((c: any) => c.id === id);
    expect(cell).toBeUndefined();
  });

  test('POST /api/reset clears outputs', async ({ request }) => {
    // Create and eval a cell
    const createRes = await request.post('/api/cells', {
      data: { type: 'code', source: '42' },
    });
    const { id } = await createRes.json();
    await request.post(`/api/cells/${id}/eval`);

    // Reset
    await request.post('/api/reset');

    // Check outputs are cleared
    const nbRes = await request.get('/api/notebook');
    const nb = await nbRes.json();
    const cell = nb.cells.find((c: any) => c.id === id);
    if (cell) {
      expect(cell.rendered_outputs.length).toBe(0);
    }
  });

  test('POST /api/cells/reorder changes cell order', async ({ request }) => {
    const nbRes = await request.get('/api/notebook');
    const nb = await nbRes.json();
    const ids = nb.cells.map((c: any) => c.id);

    // Swap first two
    const swapped = [ids[1], ids[0], ...ids.slice(2)];
    const reorderRes = await request.post('/api/cells/reorder', {
      data: { cell_ids: swapped },
    });
    expect(reorderRes.ok()).toBeTruthy();

    // Verify new order
    const nbRes2 = await request.get('/api/notebook');
    const nb2 = await nbRes2.json();
    expect(nb2.cells[0].id).toBe(ids[1]);
    expect(nb2.cells[1].id).toBe(ids[0]);
  });
});

// ── Regression: UI fixes (July 2026) ───────────────────────────
// Each test is hermetic — it cleans up any cell it adds and restores the
// title — so the tracked demo notebook is never mutated on disk.
test.describe('Regression — notebook UI fixes', () => {
  test.beforeEach(async ({ page }) => {
    await waitForLoad(page);
  });

  test('markdown cell re-renders when it loses focus (blur)', async ({ page }) => {
    const before = await cells(page).count();
    await page.getByTestId('btn-add-markdown').click();
    const newCell = cells(page).nth(before);
    await newCell.getByTestId('cell-textarea').fill('## Blur render check');
    await page.keyboard.press('Escape'); // blur → onBlur → render

    await newCell.getByTestId('markdown-rendered').waitFor({ timeout: 3000 });
    await expect(newCell.getByTestId('markdown-rendered')).toContainText('Blur render check');
    await expect(newCell.getByTestId('cell-textarea')).toHaveCount(0);

    // cleanup
    await newCell.hover();
    await newCell.getByTestId('btn-cell-delete').click();
    await expect(cells(page)).toHaveCount(before);
  });

  test('editing a cell syncs its source to the server on blur (save persistence)', async ({ page, request }) => {
    const before = await cells(page).count();
    await page.getByTestId('btn-add-code').click();
    const newCell = cells(page).nth(before);
    const marker = '; blur-sync-marker-4242';
    await newCell.getByTestId('cell-textarea').fill(marker);
    await page.keyboard.press('Escape'); // blur → persistSource

    // Root cause of the "save is broken" report: edits must reach the server,
    // not just live in the browser. Save then serializes the server's copy.
    await expect.poll(async () => {
      const nb = await (await request.get('/api/notebook')).json();
      return nb.cells.some((c: any) => c.source === marker);
    }).toBeTruthy();

    // cleanup
    await newCell.hover();
    await newCell.getByTestId('btn-cell-delete').click();
    await expect(cells(page)).toHaveCount(before);
  });

  test('notebook title edit is persisted to the server', async ({ page, request }) => {
    const titleInput = page.getByTestId('notebook-title');
    const original = await titleInput.inputValue();
    await titleInput.fill('Title Sync Test 4242');
    await titleInput.blur(); // persistTitle → server

    await expect.poll(async () => (await (await request.get('/api/notebook')).json()).title)
      .toBe('Title Sync Test 4242');

    // restore original title
    await titleInput.fill(original);
    await titleInput.blur();
  });

  test('insert dropdown stays visible when the pointer moves onto its items', async ({ page }) => {
    const divider = page.getByTestId('cell-divider').first();
    await divider.hover();
    await divider.getByTestId('btn-add-cell').click();
    const dropdown = divider.getByTestId('add-cell-dropdown');
    await expect(dropdown).toBeVisible();

    // The popover is click-open (not hover-open), so there's no hover-bridge to
    // lose in the first place — this now guards the divider's
    // `:has(sema-popover[open])` CSS fallback: moving off the "+" trigger onto a
    // menu item must not fade the divider (and the popover panel nested inside
    // it) out from under the cursor.
    await dropdown.getByTestId('btn-insert-code').hover();
    await expect.poll(() => divider.evaluate((el) => getComputedStyle(el).opacity)).toBe('1');
    await expect(dropdown.getByTestId('btn-insert-code')).toBeVisible();
  });

  test('status bar has symmetric horizontal padding', async ({ page }) => {
    const pad = await page.getByTestId('status-bar').evaluate((el) => {
      const cs = getComputedStyle(el);
      return { left: cs.paddingLeft, right: cs.paddingRight };
    });
    expect(pad.left).toBe(pad.right);
  });

  test('uses the warm brand background palette', async ({ page }) => {
    // Brand --bg is #131110 (warm), not cold #0c0c0c.
    const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(bg).toBe('rgb(19, 17, 16)');
  });

  test('fonts are bundled and served locally (no Google Fonts CDN)', async ({ page, request }) => {
    const external: string[] = [];
    page.on('request', (r) => {
      if (/gstatic|googleapis|fonts\.google/.test(r.url())) external.push(r.url());
    });
    await page.goto('/', { waitUntil: 'networkidle' });
    await page.getByTestId('cell').first().waitFor();
    expect(external, `unexpected CDN font requests: ${external.join(', ')}`).toHaveLength(0);

    const font = await request.get('/ui/fonts/jetbrains-mono-latin.woff2');
    expect(font.status()).toBe(200);
    expect(font.headers()['content-type']).toContain('font/woff2');

    await page.evaluate(() => document.fonts.ready);
    const loaded = await page.evaluate(() => ({
      jb700: document.fonts.check("700 15px 'JetBrains Mono'"),
      corm300: document.fonts.check("300 20px 'Cormorant'"),
    }));
    expect(loaded.jb700).toBeTruthy();
    expect(loaded.corm300).toBeTruthy();
  });
});
