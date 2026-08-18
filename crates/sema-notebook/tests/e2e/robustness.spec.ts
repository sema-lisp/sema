// Robustness / durability coverage for the notebook, complementing
// `ui.spec.ts` (which covers the happy paths of each feature).
//
// The question this file exists to answer is "can I trust the notebook with my
// work?", so it concentrates on the three things that erode that trust and were
// otherwise untested: does state actually reach disk, does the server stay sane
// when a client sends nonsense, and does one bad cell poison the session.
//
// Every request here goes through the same HTTP API the UI uses, so a failure
// is a real defect rather than a testing artifact.

import { test, expect } from '@playwright/test';
import {
  waitForLoad,
  createCell,
  evalCell,
  fetchNotebook,
  expectClientError,
  installNotebookRestore,
} from './helpers';

installNotebookRestore(test);

// ── Server-state durability ────────────────────────────────────
//
// The notebook saves through to its .sema-nb file. If an edit reaches the UI
// but not the file, the user loses work at the next restart — the single worst
// failure this feature can have, and the least visible.

test.describe('Server-state durability', () => {
  test('a created cell is present in the served notebook', async ({ request }) => {
    const id = await createCell(request, '(+ 1 1) ; durability probe');
    const notebook = await fetchNotebook(request);
    const found = notebook.cells.find((c: any) => c.id === id);
    expect(found, 'a created cell must appear in GET /api/notebook').toBeTruthy();
    expect(found.source).toContain('durability probe');
  });

  test('a deleted cell is gone from the served notebook', async ({ request }) => {
    const id = await createCell(request, '(+ 2 2)');
    const del = await request.delete(`/api/cells/${id}`);
    expect(del.ok()).toBeTruthy();
    const notebook = await fetchNotebook(request);
    expect(notebook.cells.find((c: any) => c.id === id)).toBeFalsy();
  });

  test('evaluation output survives a page reload', async ({ page, request }) => {
    const id = await createCell(request, '(* 6 7)');
    await evalCell(request, id);

    await waitForLoad(page);
    // Outputs are keyed by output TYPE (`cell-output-value` / `-stdout` /
    // `-error`), and cells carry no id attribute, so address the cell
    // positionally: a cell created through the API is appended last.
    const cell = page.getByTestId('cell').last();
    await expect(cell).toBeVisible();
    await expect(cell.getByTestId('cell-output-value')).toContainText('42');
  });

  test('cell order survives a page reload', async ({ page, request }) => {
    const before = await fetchNotebook(request);
    const ids = before.cells.map((c: any) => c.id);
    expect(ids.length, 'need at least two cells to reorder').toBeGreaterThan(1);

    const reordered = [ids[1], ids[0], ...ids.slice(2)];
    const res = await request.post('/api/cells/reorder', { data: { cell_ids: reordered } });
    expect(res.ok()).toBeTruthy();

    await waitForLoad(page);
    const after = await fetchNotebook(request);
    expect(after.cells.map((c: any) => c.id)).toEqual(reordered);

    await request.post('/api/cells/reorder', { data: { cell_ids: ids } });
  });

  test('unicode in source and output round-trips intact', async ({ request }) => {
    // A notebook that mangles non-ASCII on save is quietly destroying work.
    const source = '(println "héllo ✨ 日本語 🎉")';
    const id = await createCell(request, source);
    const result = await evalCell(request, id);

    const notebook = await fetchNotebook(request);
    const stored = notebook.cells.find((c: any) => c.id === id);
    expect(stored.source, 'source must survive the save/load round trip').toBe(source);

    const printed = JSON.stringify(result);
    for (const glyph of ['héllo', '✨', '日本語', '🎉']) {
      expect(printed, `output should preserve ${glyph}`).toContain(glyph);
    }
  });
});

// ── Session semantics ──────────────────────────────────────────
//
// Cells share one environment. That is the whole point of a notebook, and
// nothing verified it.

test.describe('Session semantics', () => {
  test('a definition in one cell is visible to a later cell', async ({ request }) => {
    const defId = await createCell(request, '(define shared-probe-value 1234)');
    const useId = await createCell(request, 'shared-probe-value');
    await evalCell(request, defId);
    const result = await evalCell(request, useId);
    expect(JSON.stringify(result)).toContain('1234');
  });

  test('a cell that errors does not poison later evaluations', async ({ request }) => {
    // The failure mode: one bad cell wedges the shared interpreter and every
    // subsequent cell fails too, forcing a restart.
    const badId = await createCell(request, '(this-function-does-not-exist 1)');
    const goodId = await createCell(request, '(+ 40 2)');

    const bad = await evalCell(request, badId);
    expect(JSON.stringify(bad).toLowerCase(), 'the bad cell should report an error')
      .toContain('error');

    const good = await evalCell(request, goodId);
    expect(JSON.stringify(good), 'a healthy cell must still evaluate afterwards')
      .toContain('42');
  });

  test('re-evaluating a cell reflects its edited source', async ({ request }) => {
    const id = await createCell(request, '(+ 1 1)');
    const first = await evalCell(request, id);
    expect(JSON.stringify(first)).toContain('2');

    const update = await request.post(`/api/cells/${id}`, { data: { source: '(+ 10 10)' } });
    expect(update.ok(), 'POST /api/cells/:id should update the source').toBeTruthy();

    const second = await evalCell(request, id);
    expect(JSON.stringify(second), 're-eval must use the NEW source').toContain('20');
  });

  test('editing a cell marks its output stale', async ({ request }) => {
    // `stale` is how the UI tells the user "this output no longer matches this
    // source". If it never sets, stale output looks authoritative.
    const id = await createCell(request, '(+ 3 4)');
    await evalCell(request, id);

    const evaluated = (await fetchNotebook(request)).cells.find((c: any) => c.id === id);
    expect(evaluated.stale, 'a freshly evaluated cell is not stale').toBeFalsy();

    await request.post(`/api/cells/${id}`, { data: { source: '(+ 3 5)' } });
    const edited = (await fetchNotebook(request)).cells.find((c: any) => c.id === id);
    expect(edited.stale, 'editing the source must mark the output stale').toBeTruthy();
  });
});

// ── API hardening ──────────────────────────────────────────────
//
// A local server still must not 500 or panic on malformed input: a crash takes
// the whole notebook — and any unsaved state — down with it.

test.describe('API hardening', () => {
  test('evaluating an unknown cell id is a clean client error', async ({ request }) => {
    const res = await request.post('/api/cells/no-such-cell-id/eval');
    expect(res.status(), 'unknown cell id must not 5xx').toBeLessThan(500);
    expectClientError(res.status(), 'evaluating a nonexistent cell');
  });

  test('deleting an unknown cell id is a clean client error', async ({ request }) => {
    const res = await request.delete('/api/cells/no-such-cell-id');
    expect(res.status(), 'unknown cell id must not 5xx').toBeLessThan(500);
  });

  test('creating a cell with a bogus type is rejected', async ({ request }) => {
    const res = await request.post('/api/cells', {
      data: { type: 'not-a-real-cell-type', source: '(+ 1 1)' },
    });
    expect(res.status(), 'an unknown cell type must not 5xx').toBeLessThan(500);
    expectClientError(res.status(), 'an unknown cell type');
  });

  test('creating a cell from malformed JSON is rejected', async ({ request }) => {
    const res = await request.post('/api/cells', {
      headers: { 'content-type': 'application/json' },
      data: '{ this is not json',
    });
    expect(res.status(), 'malformed JSON must not 5xx').toBeLessThan(500);
    expectClientError(res.status(), 'malformed JSON');
  });

  test('reorder with unknown ids is rejected and leaves order intact', async ({ request }) => {
    const before = await fetchNotebook(request);
    const originalOrder = before.cells.map((c: any) => c.id);

    const res = await request.post('/api/cells/reorder', {
      data: { cell_ids: ['ghost-a', 'ghost-b'] },
    });
    expect(res.status(), 'a bogus reorder must not 5xx').toBeLessThan(500);

    const after = await fetchNotebook(request);
    expect(
      after.cells.map((c: any) => c.id),
      'a rejected reorder must not drop or scramble cells'
    ).toEqual(originalOrder);
  });

  test('reorder omitting existing cells does not lose them', async ({ request }) => {
    // A partial order is the dangerous case: naive implementations rebuild the
    // list from the payload and silently discard everything left out.
    const before = await fetchNotebook(request);
    const ids = before.cells.map((c: any) => c.id);
    expect(ids.length).toBeGreaterThan(1);

    await request.post('/api/cells/reorder', { data: { cell_ids: [ids[0]] } });

    const after = await fetchNotebook(request);
    expect(
      after.cells.length,
      'a partial reorder must not delete the omitted cells'
    ).toBe(ids.length);
  });

  test('the server survives a burst of concurrent evaluations', async ({ request }) => {
    const ids = await Promise.all(
      Array.from({ length: 6 }, (_, i) => createCell(request, `(+ ${i} 100)`))
    );
    const results = await Promise.all(
      ids.map((id) => request.post(`/api/cells/${id}/eval`))
    );
    for (const [i, res] of results.entries()) {
      expect(res.status(), `concurrent eval ${i} must not 5xx`).toBeLessThan(500);
    }
    // And the server is still healthy afterwards.
    const health = await request.get('/api/notebook');
    expect(health.ok(), 'the notebook must still serve after concurrent evals').toBeTruthy();
  });

  test('a very large output does not break the server or the response', async ({ request }) => {
    const id = await createCell(request, '(string/repeat "x" 200000)');
    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status(), 'a large result must not 5xx').toBeLessThan(500);
    const health = await request.get('/api/notebook');
    expect(health.ok(), 'the server must still respond after a large output').toBeTruthy();
  });
});

// ── Rendering safety ───────────────────────────────────────────

test.describe('Rendering safety', () => {
  test('markdown cells do not execute embedded scripts', async ({ page, request }) => {
    // Markdown is rendered as HTML. A notebook file is a document users open
    // from elsewhere, so an executable payload in a markdown cell is a real
    // path to running attacker code in the page's origin.
    await createCell(
      request,
      '# probe\n\n<img src=x onerror="window.__xss_fired = true">\n',
      'markdown'
    );

    let dialogs = 0;
    page.on('dialog', async (d) => {
      dialogs += 1;
      await d.dismiss();
    });

    await waitForLoad(page);
    await page.waitForTimeout(500);

    // Positive control: prove markdown IS being rendered as HTML here, so a
    // "no payload fired" result means sanitization, not an inert page.
    const renderedHeadings = await page
      .getByTestId('cell')
      .last()
      .locator('h1')
      .count();
    expect(
      renderedHeadings,
      'the markdown cell must actually render to HTML for this test to mean anything'
    ).toBeGreaterThan(0);

    const fired = await page.evaluate(() => (window as any).__xss_fired === true);
    expect(fired, 'an onerror payload in markdown must not execute').toBeFalsy();
    expect(dialogs, 'markdown must not be able to open dialogs').toBe(0);
  });

  test('output containing HTML is shown as text, not parsed as markup', async ({
    page,
    request,
  }) => {
    const id = await createCell(request, '(println "<b>not-bold</b>")');
    await evalCell(request, id);
    await waitForLoad(page);

    const output = page.getByTestId('cell').last().getByTestId('cell-output-stdout');
    await expect(output).toBeVisible();
    await expect(output, 'the tags should be visible as literal text').toContainText(
      '<b>not-bold</b>'
    );
    expect(
      await output.locator('b').count(),
      'program output must not become live markup'
    ).toBe(0);
  });
});
