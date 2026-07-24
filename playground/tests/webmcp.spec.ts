import { expect, test, type Page } from '@playwright/test';

const TOOL_NAMES = [
  'continue_debugging',
  'find_examples',
  'format_editor',
  'get_debug_state',
  'list_files',
  'load_example',
  'read_editor',
  'read_file',
  'read_output',
  'run_editor',
  'set_breakpoints',
  'start_debugging',
  'step_debugger',
  'stop_debugging',
  'stop_run',
  'write_editor',
  'write_file',
] as const;

const READ_ONLY_TOOLS = new Set([
  'find_examples',
  'get_debug_state',
  'list_files',
  'read_editor',
  'read_file',
  'read_output',
]);

const UNTRUSTED_CONTENT_TOOLS = new Set([
  'get_debug_state',
  'list_files',
  'read_editor',
  'read_file',
  'read_output',
]);

const DESCRIPTION_TERMS: Record<string, string[]> = {
  read_editor: ['editor', 'before'],
  write_editor: ['replace', 'does not run'],
  format_editor: ['format', 'does not run'],
  run_editor: ['worker', 'wait'],
  stop_run: ['active', 'worker'],
  read_output: ['output', 'timing'],
  find_examples: ['filename', 'category', 'identifier'],
  load_example: ['replace', 'does not run'],
  list_files: ['immediate', 'non-recursive'],
  read_file: ['UTF-8', 'character'],
  write_file: ['overwrite', 'parent', '1 MiB'],
  set_breakpoints: ['replace', 'executable'],
  start_debugging: ['wait', 'pause'],
  continue_debugging: ['paused', 'breakpoint'],
  step_debugger: ['into', 'over', 'out'],
  stop_debugging: ['stop', 'idle'],
  get_debug_state: ['status', 'locals', 'stack'],
};

async function installModelContext(
  page: Page,
  rejectTool: string | null = null,
  throwSynchronously = false,
) {
  await page.addInitScript(({ rejectedName, synchronousThrow }) => {
    const tools = new Map<string, Record<string, unknown>>();
    const signals = new Set<AbortSignal>();
    const registrations: Array<{ name: string; status: string | null; hasSignal: boolean }> = [];

    Object.defineProperty(document, 'modelContext', {
      configurable: true,
      value: {
        registerTool(tool: Record<string, unknown>, options?: { signal?: AbortSignal }) {
          const name = String(tool.name);
          registrations.push({
            name,
            status: document.getElementById('status')?.textContent ?? null,
            hasSignal: options?.signal instanceof AbortSignal,
          });
          if (options?.signal) signals.add(options.signal);
          if (name === rejectedName) {
            const error = new Error(`registration rejected for ${name}`);
            if (synchronousThrow) throw error;
            return Promise.reject(error);
          }
          tools.set(name, tool);
          options?.signal?.addEventListener('abort', () => tools.delete(name), { once: true });
          return Promise.resolve();
        },
        async getTools() {
          return [...tools.values()];
        },
        async executeTool(tool: Record<string, unknown>, params: string) {
          const execute = tool.execute as (input: unknown) => unknown;
          return execute(JSON.parse(params));
        },
      },
    });

    Object.defineProperty(window, '__webMcpTest', {
      value: { tools, registrations, signals },
    });
  }, { rejectedName: rejectTool, synchronousThrow: throwSynchronously });
}

async function executeTool<T>(page: Page, name: string, input: Record<string, unknown> = {}) {
  return page.evaluate(async ({ toolName, params }) => {
    const state = (window as unknown as {
      __webMcpTest: { tools: Map<string, Record<string, unknown>> };
    }).__webMcpTest;
    const tool = state.tools.get(toolName);
    if (!tool) throw new Error(`WebMCP tool not registered: ${toolName}`);
    const execute = tool.execute as (value: Record<string, unknown>) => Promise<unknown>;
    return execute(params);
  }, { toolName: name, params: input }) as Promise<T>;
}

test('registers the complete playground tool surface before WASM is ready', async ({ page }) => {
  await installModelContext(page);
  await page.goto('/');

  await expect.poll(() => page.evaluate(() => {
    const state = (window as unknown as {
      __webMcpTest: { tools: Map<string, Record<string, unknown>> };
    }).__webMcpTest;
    return state.tools.size;
  })).toBe(TOOL_NAMES.length);

  const registrations = await page.evaluate(() => {
    const state = (window as unknown as {
      __webMcpTest: {
        tools: Map<string, Record<string, unknown>>;
        registrations: Array<{ name: string; status: string | null; hasSignal: boolean }>;
        signals: Set<AbortSignal>;
      };
    }).__webMcpTest;
    return {
      registrations: state.registrations,
      signalCount: state.signals.size,
      descriptors: [...state.tools.values()].map((tool) => ({
        name: tool.name,
        description: tool.description,
        inputSchema: tool.inputSchema,
        annotations: tool.annotations,
        hasExecute: typeof tool.execute === 'function',
      })),
    };
  });

  expect(registrations.registrations.map(({ name }) => name).sort()).toEqual([...TOOL_NAMES]);
  expect(registrations.registrations.every(({ hasSignal }) => hasSignal)).toBe(true);
  expect(registrations.signalCount).toBe(1);
  expect(registrations.registrations.every(({ status }) => status !== 'Ready')).toBe(true);

  for (const descriptor of registrations.descriptors) {
    expect(descriptor.name).toMatch(/^[a-z][a-z0-9_]{0,29}$/);
    expect(descriptor.description).toEqual(expect.any(String));
    expect((descriptor.description as string).length).toBeGreaterThan(10);
    expect((descriptor.description as string).length).toBeLessThanOrEqual(500);
    for (const term of DESCRIPTION_TERMS[String(descriptor.name)] ?? []) {
      expect(String(descriptor.description).toLowerCase()).toContain(term.toLowerCase());
    }
    expect(descriptor.inputSchema).toMatchObject({ type: 'object' });
    for (const property of Object.values(
      (descriptor.inputSchema as { properties?: Record<string, { description?: string }> }).properties ?? {},
    )) {
      expect(property.description).toEqual(expect.any(String));
      expect(property.description!.length).toBeLessThanOrEqual(150);
    }
    expect(descriptor.annotations).toEqual({
      readOnlyHint: READ_ONLY_TOOLS.has(String(descriptor.name)),
      ...(UNTRUSTED_CONTENT_TOOLS.has(String(descriptor.name))
        ? { untrustedContentHint: true }
        : {}),
    });
    expect(descriptor.hasExecute).toBe(true);
  }
});

test('aborts all registrations when the browser rejects one tool', async ({ page }) => {
  await installModelContext(page, 'format_editor');
  await page.goto('/');

  await expect.poll(() => page.evaluate(() => {
    const state = (window as unknown as {
      __webMcpTest: { signals: Set<AbortSignal> };
    }).__webMcpTest;
    return [...state.signals].every((signal) => signal.aborted);
  })).toBe(true);
  expect(await page.evaluate(() => {
    const state = (window as unknown as {
      __webMcpTest: { tools: Map<string, Record<string, unknown>> };
    }).__webMcpTest;
    return state.tools.size;
  })).toBe(0);
});

test('aborts earlier registrations when registerTool throws synchronously', async ({ page }) => {
  await installModelContext(page, 'format_editor', true);
  await page.goto('/');

  await expect.poll(() => page.evaluate(() => {
    const state = (window as unknown as {
      __webMcpTest: { signals: Set<AbortSignal> };
    }).__webMcpTest;
    return [...state.signals].every((signal) => signal.aborted);
  })).toBe(true);
  expect(await page.evaluate(() => {
    const state = (window as unknown as {
      __webMcpTest: { tools: Map<string, Record<string, unknown>> };
    }).__webMcpTest;
    return state.tools.size;
  })).toBe(0);
});

test('keeps the normal playground usable when WebMCP is unsupported', async ({ page }) => {
  const pageErrors: string[] = [];
  const warnings: string[] = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'warning') warnings.push(message.text());
  });

  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });
  expect(await page.evaluate(() => 'modelContext' in document)).toBe(false);
  await page.getByTestId('editor').fill('(+ 40 2)');
  await page.getByTestId('run-btn').click();
  await expect(page.getByTestId('output')).toContainText('=> 42');
  expect(pageErrors).toEqual([]);
  expect(warnings.filter((warning) => warning.includes('WebMCP'))).toEqual([]);
});

test('edits, formats, runs, and pages playground source and output', async ({ page }) => {
  await installModelContext(page);
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });

  const initial = await executeTool<{
    ok: boolean;
    content: string;
    offset: number;
    total: number;
    truncated: boolean;
  }>(page, 'read_editor');
  expect(initial).toMatchObject({ ok: true, offset: 0, total: expect.any(Number) });

  const source = '(define (square x)(* x x))\n(square 7)';
  await expect(executeTool(page, 'write_editor', { code: source })).resolves.toMatchObject({
    ok: true,
    length: source.length,
  });
  await expect(page.getByTestId('editor')).toHaveValue(source);

  const pageOne = await executeTool<{
    ok: boolean;
    content: string;
    offset: number;
    total: number;
    truncated: boolean;
  }>(page, 'read_editor', { limit: 24 });
  expect(pageOne).toMatchObject({ ok: true, offset: 0, total: source.length, truncated: true });
  expect(pageOne.content).toHaveLength(24);

  await expect(executeTool(page, 'format_editor')).resolves.toMatchObject({
    ok: true,
    formatted: true,
  });
  await expect(page.getByTestId('editor')).toHaveValue(
    '(define (square x) (* x x))\n(square 7)\n',
  );

  await expect(executeTool(page, 'run_editor')).resolves.toMatchObject({
    ok: true,
    status: 'finished',
    value: '49',
  });

  const output = await executeTool<{
    ok: boolean;
    content: string;
    truncated: boolean;
  }>(page, 'read_output', { limit: 12 });
  expect(output).toMatchObject({ ok: true, truncated: true });
  expect(output.content).toHaveLength(12);
  expect(await executeTool(page, 'read_output', { offset: 12, limit: 12000 })).toMatchObject({
    ok: true,
    offset: 12,
    truncated: false,
    next_offset: null,
  });
});

test('finds examples and performs bounded text-only VFS operations', async ({ page }) => {
  await installModelContext(page);
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });

  const found = await executeTool<{
    ok: boolean;
    examples: Array<{ id: string; name: string; category: string }>;
    total: number;
  }>(page, 'find_examples', { query: 'maze', limit: 5 });
  expect(found).toMatchObject({
    ok: true,
    total: 4,
  });
  expect(found.examples).toEqual(expect.arrayContaining([{
      id: 'visuals/maze.sema',
      name: 'maze.sema',
      category: 'Visuals',
  }]));

  const visuals = await executeTool<{
    ok: boolean;
    examples: Array<{ id: string; name: string; category: string }>;
    total: number;
    truncated: boolean;
  }>(page, 'find_examples', { query: 'visuals' });
  expect(visuals).toMatchObject({ ok: true, total: 11, truncated: false });
  expect(visuals.examples).toHaveLength(11);

  const limited = await executeTool<{
    ok: boolean;
    examples: Array<{ id: string }>;
    total: number;
    truncated: boolean;
  }>(page, 'find_examples', { query: 'visuals', limit: 3 });
  expect(limited).toMatchObject({ ok: true, total: 11, truncated: true });
  expect(limited.examples).toHaveLength(3);

  await expect(executeTool(page, 'load_example', { id: 'visuals/maze.sema' })).resolves.toMatchObject({
    ok: true,
    id: 'visuals/maze.sema',
    name: 'maze.sema',
  });
  await expect(page.getByTestId('editor')).toHaveValue(/generate-maze/);

  const content = '(define answer 42)\n(println answer)\n';
  await expect(executeTool(page, 'write_file', {
    path: '/agent/demo.sema',
    content,
  })).resolves.toMatchObject({
    ok: true,
    path: '/agent/demo.sema',
    bytes: content.length,
  });

  await expect(executeTool(page, 'list_files', { dir: '/' })).resolves.toMatchObject({
    ok: true,
    dir: '/',
    entries: [{ name: 'agent', path: '/agent', type: 'directory' }],
  });
  await expect(executeTool(page, 'list_files', { dir: '/agent' })).resolves.toMatchObject({
    ok: true,
    dir: '/agent',
    entries: [{ name: 'demo.sema', path: '/agent/demo.sema', type: 'file' }],
  });
  await expect(page.getByTestId('file-tree')).toContainText('demo.sema');

  await expect(executeTool(page, 'read_file', {
    path: '/agent/demo.sema',
    offset: 7,
    limit: 10,
  })).resolves.toMatchObject({
    ok: true,
    path: '/agent/demo.sema',
    content: ' answer 42',
    offset: 7,
    total: content.length,
    truncated: true,
    next_offset: 17,
  });

  await expect(executeTool(page, 'write_file', {
    path: '/agent/../escape.sema',
    content: 'no',
  })).resolves.toEqual({
    ok: false,
    error: { code: 'INVALID_PATH', message: expect.any(String) },
  });
});

test('drives the debugger and waits for each stable state', async ({ page }) => {
  await installModelContext(page);
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });

  const source = '(+ 1 2)\n(+ 3 4)';
  await executeTool(page, 'write_editor', { code: source });
  await expect(executeTool(page, 'set_breakpoints', { lines: [] })).resolves.toEqual({
    ok: true,
    breakpoints: [],
  });

  await expect(executeTool(page, 'start_debugging')).resolves.toMatchObject({
    ok: true,
    state: 'paused',
    line: 1,
    breakpoints: [],
  });

  const paused = await executeTool<{
    ok: boolean;
    state: string;
    line: number;
    locals: unknown[];
    stack: Array<{ name: string; line: number; column: number }>;
  }>(page, 'get_debug_state');
  expect(paused).toMatchObject({
    ok: true,
    state: 'paused',
    line: 1,
    locals: expect.any(Array),
    stack: expect.any(Array),
  });

  await expect(executeTool(page, 'step_debugger', { mode: 'over' })).resolves.toMatchObject({
    ok: true,
    state: 'paused',
    line: 2,
  });

  await expect(executeTool(page, 'continue_debugging')).resolves.toMatchObject({
    ok: true,
    state: 'idle',
    status: 'finished',
    value: '7',
  });
  await expect(page.getByTestId('status')).toHaveText('Ready');

  await expect(executeTool(page, 'step_debugger', { mode: 'sideways' })).resolves.toEqual({
    ok: false,
    error: { code: 'INVALID_INPUT', message: expect.any(String) },
  });
  await expect(executeTool(page, 'stop_debugging')).resolves.toEqual({
    ok: false,
    error: { code: 'NOT_DEBUGGING', message: expect.any(String) },
  });
});

test('guards concurrent mutations and cancels a worker-backed run', async ({ page }) => {
  await installModelContext(page);
  await page.goto('/?worker');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 20000 });

  const source = '(await (async (async/sleep 5000) (println "should not print")))';
  await executeTool(page, 'write_editor', { code: source });
  await page.evaluate(() => {
    const state = (window as unknown as {
      __webMcpTest: { tools: Map<string, Record<string, unknown>> };
    }).__webMcpTest;
    const run = state.tools.get('run_editor')?.execute as (
      input: Record<string, never>,
    ) => Promise<unknown>;
    (window as unknown as { __webMcpRun: Promise<unknown> }).__webMcpRun = run({});
  });

  await expect(page.getByTestId('run-btn')).toContainText('Stop', { timeout: 5000 });
  await expect(executeTool(page, 'write_editor', { code: '(+ 1 2)' })).resolves.toEqual({
    ok: false,
    error: { code: 'BUSY', message: expect.any(String) },
  });
  await expect(executeTool(page, 'stop_run')).resolves.toEqual({
    ok: true,
    status: 'stopping',
  });

  const cancelled = await page.evaluate(() =>
    (window as unknown as { __webMcpRun: Promise<unknown> }).__webMcpRun
  );
  expect(cancelled).toEqual({
    ok: false,
    error: { code: 'CANCELLED', message: expect.any(String) },
  });
  await expect(page.getByTestId('status')).toHaveText('Stopped');
  await expect(page.getByTestId('output')).not.toContainText('should not print');

  await executeTool(page, 'write_editor', { code: '(+ 20 22)' });
  await expect(executeTool(page, 'run_editor')).resolves.toMatchObject({
    ok: true,
    value: '42',
  });
});
