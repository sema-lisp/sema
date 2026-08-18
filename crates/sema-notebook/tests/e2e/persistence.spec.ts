// Durability against the FILE, plus the endpoints nothing else exercises.
//
// `robustness.spec.ts` asserts through `GET /api/notebook`, which is the
// server's in-memory state. That is not what survives a restart — the
// `.sema-nb` file is. These tests read that file directly, so "my work is safe"
// is verified against the artifact the user actually keeps.
//
// Also covers `/api/save`, `/api/eval-all`, `/api/env` and undo depth, none of
// which had coverage.

import { test, expect } from '@playwright/test';
import { createCell, fetchNotebook, readNotebookFile, installNotebookRestore } from './helpers';

// Shares one long-lived server with the other specs, which assert exact cell
// counts and "undo is disabled initially" — so this suite must put everything
// back, including on disk, since it saves.
installNotebookRestore(test, { restoreTitle: true, save: true });

test.describe('On-disk durability', () => {
  test('a saved cell is present in the .sema-nb file', async ({ request }) => {
    const marker = `on-disk-probe-${Date.now()}`;
    const id = await createCell(request, `(+ 1 1) ; ${marker}`);

    const save = await request.post('/api/save');
    expect(save.ok(), 'POST /api/save should succeed').toBeTruthy();

    const onDisk = readNotebookFile();
    const stored = onDisk.cells.find((c: any) => c.id === id);
    expect(stored, 'the cell must exist in the file, not just in memory').toBeTruthy();
    expect(stored.source).toContain(marker);
  });

  test('the saved file stays valid and round-trippable', async ({ request }) => {
    await createCell(request, '(println "round trip")');
    await request.post('/api/save');

    // Parsing already happened in readNotebookFile; assert the shape the loader
    // requires, so a corrupt-but-parseable write is still caught.
    const onDisk = readNotebookFile();
    expect(onDisk.version, 'the format version must be written').toBeTruthy();
    expect(Array.isArray(onDisk.cells)).toBeTruthy();
    for (const cell of onDisk.cells) {
      expect(cell.id, 'every cell needs an id').toBeTruthy();
      expect(typeof cell.source, 'every cell needs a source string').toBe('string');
      expect(['code', 'markdown']).toContain(cell.type);
    }
    // And the server can load what it wrote: ids on disk match the live set.
    const live = await fetchNotebook(request);
    expect(onDisk.cells.map((c: any) => c.id)).toEqual(live.cells.map((c: any) => c.id));
  });

  test('a title change reaches the file after a save', async ({ request }) => {
    const title = `Persistence Probe ${Date.now()}`;
    const res = await request.post('/api/title', { data: { title } });
    expect(res.ok()).toBeTruthy();
    await request.post('/api/save');

    expect(readNotebookFile().metadata.title).toBe(title);
  });

  test('unicode survives the write to disk byte-for-byte', async ({ request }) => {
    const source = '(println "diskette ✨ 日本語 🎉 ünïcode")';
    const id = await createCell(request, source);
    await request.post('/api/save');

    const stored = readNotebookFile().cells.find((c: any) => c.id === id);
    expect(stored.source, 'the file must hold the exact source').toBe(source);
  });

  test('edits are NOT on disk until a save — the durability boundary', async ({ request }) => {
    // Documents real behavior rather than wishing for it: there is no autosave,
    // so everything since the last `/api/save` is lost if the process dies.
    // If autosave is ever added, this test SHOULD fail and be rewritten — that
    // is the point of pinning it.
    const marker = `unsaved-probe-${Date.now()}`;
    const id = await createCell(request, `(+ 0 0) ; ${marker}`);

    const onDisk = readNotebookFile();
    expect(
      onDisk.cells.find((c: any) => c.id === id),
      'a cell created without an explicit save is in memory only'
    ).toBeFalsy();

    // And a save makes it durable, so the boundary is exactly the save call.
    await request.post('/api/save');
    expect(readNotebookFile().cells.find((c: any) => c.id === id)).toBeTruthy();
  });
});

test.describe('Undo depth', () => {
  test('undo is rejected cleanly when there is nothing to undo', async ({ request }) => {
    await request.post('/api/reset');
    const res = await request.post('/api/undo');
    expect(res.status(), 'an empty undo stack must not 5xx').toBeLessThan(500);
  });

  test('undo is single-level: one snapshot, then a clean refusal', async ({ request }) => {
    // The engine keeps `snapshot: Option<Snapshot>` — exactly ONE evaluation can
    // be undone, not a stack. Pinned so that stays a deliberate choice: if undo
    // ever becomes multi-level this test should fail and be rewritten.
    await request.post('/api/reset');
    const first = await createCell(request, '(+ 1 1)');
    const second = await createCell(request, '(+ 2 2)');
    await request.post(`/api/cells/${first}/eval`);
    await request.post(`/api/cells/${second}/eval`);

    const undoOne = await request.post('/api/undo');
    expect(undoOne.ok()).toBeTruthy();
    const body = await undoOne.json();
    expect(body.undone_cell_id, 'undo applies to the most recent evaluation').toBe(second);
    expect(body.can_undo, 'only the latest evaluation is retained').toBeFalsy();

    // The earlier evaluation is NOT recoverable, and asking is a clean refusal.
    const undoTwo = await request.post('/api/undo');
    expect(undoTwo.ok(), 'a second undo is refused').toBeFalsy();
    expect(undoTwo.status(), 'refusing must not 5xx').toBeLessThan(500);
  });
});

test.describe('Whole-notebook evaluation', () => {
  test('eval-all returns a result per code cell', async ({ request }) => {
    const res = await request.post('/api/eval-all');
    expect(res.ok(), 'POST /api/eval-all should succeed').toBeTruthy();
    const results = await res.json();
    expect(Array.isArray(results)).toBeTruthy();

    const nb = await fetchNotebook(request);
    const codeCells = nb.cells.filter((c: any) => c.cell_type === 'code');
    expect(
      results.length,
      'every code cell should be evaluated'
    ).toBe(codeCells.length);
  });

  test('one failing cell does not abort the rest of eval-all', async ({ request }) => {
    await createCell(request, '(this-is-not-defined 1)');
    const sentinel = await createCell(request, '(+ 21 21)');

    const res = await request.post('/api/eval-all');
    expect(res.status(), 'a failing cell must not 5xx the run').toBeLessThan(500);

    const nb = await fetchNotebook(request);
    const cell = nb.cells.find((c: any) => c.id === sentinel);
    expect(
      JSON.stringify(cell.rendered_outputs),
      'a cell after the failing one must still have produced its result'
    ).toContain('42');
  });
});

test.describe('Environment inspection', () => {
  test('env reflects a definition made by a cell', async ({ request }) => {
    const name = `env-probe-${Date.now()}`.replace(/[^a-z0-9-]/g, '-');
    const id = await createCell(request, `(define ${name} 4242)`);
    await request.post(`/api/cells/${id}/eval`);

    const res = await request.get('/api/env');
    expect(res.ok(), 'GET /api/env should succeed').toBeTruthy();
    const { bindings } = await res.json();
    expect(bindings, 'env must expose the bindings map').toBeTruthy();
    expect(Object.keys(bindings)).toContain(name);
    expect(String(bindings[name])).toContain('4242');
  });

  test('reset clears definitions from the environment', async ({ request }) => {
    const name = `reset-probe-${Date.now()}`.replace(/[^a-z0-9-]/g, '-');
    const id = await createCell(request, `(define ${name} 1)`);
    await request.post(`/api/cells/${id}/eval`);
    expect(Object.keys((await (await request.get('/api/env')).json()).bindings)).toContain(name);

    await request.post('/api/reset');

    const after = await (await request.get('/api/env')).json();
    expect(
      Object.keys(after.bindings),
      'reset must clear the session environment, not just the outputs'
    ).not.toContain(name);
  });
});

test.describe('Cell type conversion', () => {
  test('a code cell can be converted to markdown and back', async ({ request }) => {
    const id = await createCell(request, '(+ 1 1)');

    const toMd = await request.post(`/api/cells/${id}`, {
      data: { source: '# now markdown', type: 'markdown' },
    });
    expect(toMd.ok(), 'converting to markdown should succeed').toBeTruthy();
    let cell = (await fetchNotebook(request)).cells.find((c: any) => c.id === id);
    expect(cell.cell_type).toBe('markdown');

    const toCode = await request.post(`/api/cells/${id}`, {
      data: { source: '(+ 2 2)', type: 'code' },
    });
    expect(toCode.ok(), 'converting back to code should succeed').toBeTruthy();
    cell = (await fetchNotebook(request)).cells.find((c: any) => c.id === id);
    expect(cell.cell_type).toBe('code');

    // And it is genuinely a code cell again — it evaluates.
    const evaluated = await request.post(`/api/cells/${id}/eval`);
    expect(JSON.stringify(await evaluated.json())).toContain('4');
  });
});
