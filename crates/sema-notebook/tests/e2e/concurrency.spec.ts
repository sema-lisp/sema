// Concurrency, insertion position, and pathological input.
//
// The other suites drive one request at a time against well-formed input. This
// one overlaps requests and feeds the server things a real notebook eventually
// contains — a very large cell, an empty cell, a cell deleted while it is being
// evaluated. The bar throughout is the same: never a 5xx, never a hang, and the
// notebook still coherent afterwards.

import { test, expect } from '@playwright/test';
import { createCell, fetchNotebook, installNotebookRestore } from './helpers';

installNotebookRestore(test);

test.describe('Overlapping requests', () => {
  test('a reset issued during an evaluation leaves the server healthy', async ({ request }) => {
    // A slow cell so the reset genuinely lands mid-flight.
    const slow = await createCell(request, '(foldl + 0 (range 1 400000))');
    const [evalRes, resetRes] = await Promise.all([
      request.post(`/api/cells/${slow}/eval`),
      request.post('/api/reset'),
    ]);

    expect(evalRes.status(), 'the eval must not 5xx').toBeLessThan(500);
    expect(resetRes.status(), 'the reset must not 5xx').toBeLessThan(500);
    expect((await request.get('/api/notebook')).ok(), 'the server survives').toBeTruthy();
  });

  test('deleting a cell while it evaluates does not corrupt the notebook', async ({ request }) => {
    const doomed = await createCell(request, '(foldl + 0 (range 1 400000))');
    const before = (await fetchNotebook(request)).cells.length;

    const [evalRes, delRes] = await Promise.all([
      request.post(`/api/cells/${doomed}/eval`),
      request.delete(`/api/cells/${doomed}`),
    ]);
    expect(evalRes.status()).toBeLessThan(500);
    expect(delRes.status()).toBeLessThan(500);

    const after = await fetchNotebook(request);
    // Whichever order the two landed in, exactly one cell is gone and every
    // remaining cell is still well-formed.
    expect(after.cells.length).toBe(before - 1);
    expect(after.cells.find((c: any) => c.id === doomed)).toBeFalsy();
    for (const cell of after.cells) {
      expect(cell.id).toBeTruthy();
      expect(typeof cell.source).toBe('string');
    }
  });

  test('eval-all overlapping a single eval keeps both responses sane', async ({ request }) => {
    const one = await createCell(request, '(+ 5 5)');
    const [single, all] = await Promise.all([
      request.post(`/api/cells/${one}/eval`),
      request.post('/api/eval-all'),
    ]);
    expect(single.status()).toBeLessThan(500);
    expect(all.status()).toBeLessThan(500);
    expect((await request.get('/api/notebook')).ok()).toBeTruthy();
  });

  test('concurrent edits to the same cell leave one of the two sources', async ({ request }) => {
    const id = await createCell(request, '(+ 0 0)');
    await Promise.all([
      request.post(`/api/cells/${id}`, { data: { source: '(+ 1 1)' } }),
      request.post(`/api/cells/${id}`, { data: { source: '(+ 2 2)' } }),
    ]);

    const cell = (await fetchNotebook(request)).cells.find((c: any) => c.id === id);
    // Last-writer-wins is fine; a torn or empty source is not.
    expect(
      ['(+ 1 1)', '(+ 2 2)'],
      'the surviving source must be one of the two writes, intact'
    ).toContain(cell.source);
  });
});

test.describe('Insertion position', () => {
  test('`after` inserts directly below the named cell', async ({ request }) => {
    const anchor = await createCell(request, '; anchor');
    const inserted = await createCell(request, '; inserted', 'code', anchor);

    const ids = (await fetchNotebook(request)).cells.map((c: any) => c.id);
    expect(
      ids[ids.indexOf(anchor) + 1],
      'the new cell belongs immediately after its anchor'
    ).toBe(inserted);
  });

  test('an unknown `after` id is handled without a 5xx', async ({ request }) => {
    const res = await request.post('/api/cells', {
      data: { type: 'code', source: '; orphan anchor', after: 'no-such-anchor' },
    });
    expect(res.status(), 'a bogus anchor must not 5xx').toBeLessThan(500);
    if (res.ok()) {
      const { id } = await res.json();
      // However it resolves, the cell must be reachable exactly once.
      const ids = (await fetchNotebook(request)).cells.map((c: any) => c.id);
      expect(ids.filter((x: string) => x === id).length).toBe(1);
    }
  });
});

test.describe('Pathological input', () => {
  test('an empty cell evaluates without erroring the server', async ({ request }) => {
    const id = await createCell(request, '');
    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status(), 'an empty cell must not 5xx').toBeLessThan(500);
    expect((await request.get('/api/notebook')).ok()).toBeTruthy();
  });

  test('a whitespace-only cell evaluates without erroring the server', async ({ request }) => {
    const id = await createCell(request, '   \n\t  \n');
    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status()).toBeLessThan(500);
  });

  test('evaluating a markdown cell is refused or inert, never a 5xx', async ({ request }) => {
    const id = await createCell(request, '# not code', 'markdown');
    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status(), 'evaluating markdown must not 5xx').toBeLessThan(500);
    expect((await request.get('/api/notebook')).ok()).toBeTruthy();
  });

  test('a very large markdown cell round-trips', async ({ request }) => {
    // ~400KB of markdown: notebooks accumulate long prose and pasted output.
    const big = ('lorem ipsum dolor sit amet '.repeat(400) + '\n\n').repeat(40);
    const id = await createCell(request, big, 'markdown');

    const cell = (await fetchNotebook(request)).cells.find((c: any) => c.id === id);
    expect(cell.source.length, 'the whole cell must survive').toBe(big.length);
  });

  test('moderately nested expressions evaluate normally', async ({ request }) => {
    const depth = 50;
    const source = '(+ 1 '.repeat(depth) + '0' + ')'.repeat(depth);
    const id = await createCell(request, source);
    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status()).toBeLessThan(500);
    expect(JSON.stringify(await res.json()), 'it should sum to the nesting depth').toContain(
      String(depth)
    );
  });

  test('nesting past the resolver limit errors instead of killing the server', async ({
    request,
  }) => {
    // REGRESSION. `MAX_RESOLVE_DEPTH` (256) is calibrated against a main-
    // thread-sized stack, but the notebook engine ran on a default ~2MiB
    // thread, so a cell nested only ~200 deep overflowed the native stack and
    // aborted the whole process — "fatal runtime error: stack overflow" —
    // taking every unsaved cell with it, since saving is explicit. The engine
    // thread now gets 16MiB, so the guard reports the error first.
    const depth = 200;
    const source = '(+ 1 '.repeat(depth) + '0' + ')'.repeat(depth);
    const id = await createCell(request, source);

    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status(), 'must not 5xx').toBeLessThan(500);
    expect(
      JSON.stringify(await res.json()).toLowerCase(),
      'the depth guard should report, not the stack'
    ).toContain('depth');

    const health = await request.get('/api/notebook');
    expect(health.ok(), 'THE SERVER MUST STILL BE ALIVE').toBeTruthy();
  });

  test('a cell that exceeds the reader depth limit fails cleanly', async ({ request }) => {
    const depth = 5000;
    const source = '(+ 1 '.repeat(depth) + '0' + ')'.repeat(depth);
    const id = await createCell(request, source);
    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status(), 'over-deep input must not 5xx or kill the process').toBeLessThan(500);
    expect((await request.get('/api/notebook')).ok(), 'the server survives').toBeTruthy();
  });

  test('a cell producing a large output keeps the notebook responsive', async ({ request }) => {
    const id = await createCell(request, '(println (string/repeat "x" 300000))');
    const res = await request.post(`/api/cells/${id}/eval`);
    expect(res.status()).toBeLessThan(500);

    const health = await request.get('/api/notebook');
    expect(health.ok(), 'the notebook still serves after a large output').toBeTruthy();
  });
});
