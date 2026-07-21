import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

import { expect, test } from '@playwright/test';

const playgroundRoot = resolve(__dirname, '..');
const repositoryRoot = resolve(playgroundRoot, '..');
const catalogPath = resolve(repositoryRoot, 'website/public/.well-known/ai-catalog.json');

const nativeMcpTools = [
  'build',
  'compile',
  'disasm',
  'docs',
  'docs_search',
  'eval',
  'fmt',
  'info',
  'notebook/add_cell',
  'notebook/delete_cell',
  'notebook/eval_all',
  'notebook/eval_cell',
  'notebook/export',
  'notebook/new',
  'notebook/read',
  'notebook/update_cell',
  'run_file',
];

test('playground advertises WebMCP and the canonical AI catalog before scripts run', async ({ request }) => {
  const response = await request.get('/');
  expect(response.ok()).toBeTruthy();

  const html = await response.text();
  const originTrial = html.match(
    /<meta\s+http-equiv="origin-trial"\s+content="([^"]+)"\s*\/?>/i,
  );
  expect(originTrial).not.toBeNull();

  const token = Buffer.from(originTrial![1], 'base64');
  const payloadStart = token.indexOf('{'.charCodeAt(0));
  expect(payloadStart).toBeGreaterThanOrEqual(0);
  const payload = JSON.parse(token.subarray(payloadStart).toString('utf8'));
  expect(payload).toMatchObject({
    origin: 'https://sema.run:443',
    feature: 'WebMCP',
  });
  expect(payload.expiry * 1000).toBeGreaterThan(Date.now() + 30 * 24 * 60 * 60 * 1000);

  const catalogLink =
    '<link rel="ai-catalog" href="https://sema-lang.com/.well-known/ai-catalog.json" type="application/ai-catalog+json">';
  expect(html).toContain(catalogLink);
  expect(html.indexOf(originTrial![0])).toBeLessThan(html.indexOf('<script'));
  expect(html.indexOf(catalogLink)).toBeLessThan(html.indexOf('<script'));
});

test('playground discovery files point agents to the shared Sema surfaces', async ({ request }) => {
  const llmsResponse = await request.get('/llms.txt');
  expect(llmsResponse.ok()).toBeTruthy();
  expect(llmsResponse.headers()['content-type']).toContain('text/plain');
  const llms = await llmsResponse.text();
  expect(llms.length).toBeGreaterThan(500);
  expect(llms).toMatch(/^# Sema Playground$/m);
  expect(llms).toContain('https://sema-lang.com/docs/for-agents.md');
  expect(llms).toContain('https://sema-lang.com/llms.txt');
  expect(llms).toContain('https://sema-lang.com/.well-known/ai-catalog.json');

  const robotsResponse = await request.get('/robots.txt');
  expect(robotsResponse.ok()).toBeTruthy();
  expect(await robotsResponse.text()).toContain(
    'Agentmap: https://sema-lang.com/.well-known/ai-catalog.json',
  );
});

test('playground responses enable the isolation headers required by its worker runtime', async ({ request }) => {
  const response = await request.get('/');
  const headers = response.headers();
  expect(headers['cross-origin-opener-policy']).toBe('same-origin');
  expect(headers['cross-origin-embedder-policy']).toBe('require-corp');
  expect(headers['origin-agent-cluster']).toBe('?1');
});

test('canonical AI catalog describes every Sema agent surface without volatile metadata', async () => {
  const catalog = JSON.parse(await readFile(catalogPath, 'utf8'));
  expect(catalog.specVersion).toBe('1.0');
  expect(catalog.host).toEqual({
    displayName: 'Sema',
    documentationUrl: 'https://sema-lang.com/docs/',
  });

  const expectedEntries = new Map([
    ['urn:air:sema-lang.com:mcp:sema', 'application/mcp-server-card+json'],
    ['urn:air:sema-lang.com:skill:sema-agents', 'application/ai-skill+md'],
    ['urn:air:sema-lang.com:docs:llms-index', 'text/plain'],
    ['urn:air:sema-lang.com:webmcp:playground', 'text/html'],
  ]);
  expect(catalog.entries).toHaveLength(expectedEntries.size);

  for (const entry of catalog.entries) {
    expect(entry.type).toBe(expectedEntries.get(entry.identifier));
    expect(Number('url' in entry) + Number('data' in entry)).toBe(1);
    expect(entry.representativeQueries.length).toBeGreaterThanOrEqual(2);
    expect(entry.representativeQueries.length).toBeLessThanOrEqual(5);
    expect(entry).not.toHaveProperty('version');
    expect(entry).not.toHaveProperty('updatedAt');
  }

  const mcp = catalog.entries.find(
    (entry: { identifier: string }) => entry.identifier === 'urn:air:sema-lang.com:mcp:sema',
  );
  expect(mcp.data.transport).toEqual({ type: 'stdio', command: 'sema', args: ['mcp'] });
  expect(mcp.data.tools.map((tool: { name: string }) => tool.name).sort()).toEqual(nativeMcpTools);
  expect(mcp.data.dynamicTools).toContain('deftool');

  const websiteRobots = await readFile(resolve(repositoryRoot, 'website/public/robots.txt'), 'utf8');
  expect(websiteRobots).toContain(
    'Agentmap: https://sema-lang.com/.well-known/ai-catalog.json',
  );
  const websiteConfig = await readFile(resolve(repositoryRoot, 'website/.vitepress/config.ts'), 'utf8');
  expect(websiteConfig).toContain("rel: 'ai-catalog'");
  expect(websiteConfig).toContain("href: '/.well-known/ai-catalog.json'");
});
