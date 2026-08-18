import { test, expect, Page } from '@playwright/test';
import { waitForReady } from './helpers';

async function clickRun(page: Page) {
  await page.getByTestId('run-btn').click();
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 30000 });
}

test('VFS: run file-creating script and check file tree', async ({ page }) => {
  await waitForReady(page);

  await page.getByTestId('editor').fill(`
(file/mkdir "test-dir")
(file/write "test-dir/hello.txt" "Hello from VFS!")
(println "done")
`);

  await clickRun(page);

  // File tree is always visible in the files panel. It dogfoods <sema-tree>
  // (from @sema-lang/ui, not this repo) — entries are <sema-tree-item> nodes
  // whose visible label is their `label` attribute value, so getByText locates
  // them without depending on that internal attribute selector.
  const fileTree = page.getByTestId('file-tree');
  await expect(fileTree).toBeVisible();
  await expect(fileTree.getByText('test-dir', { exact: true })).toBeVisible();
  await expect(fileTree.getByText('hello.txt', { exact: true })).toBeVisible();
});

test('VFS: clicking a file shows content in preview pane', async ({ page }) => {
  await waitForReady(page);

  await page.getByTestId('editor').fill(`
(file/write "greeting.txt" "Hello World!")
`);
  await clickRun(page);

  // Click on the file in the tree (a <sema-tree-item> leaf from @sema-lang/ui;
  // getByText matches its visible label rather than the internal attribute).
  const fileEntry = page.getByTestId('file-tree').getByText('greeting.txt', { exact: true });
  await fileEntry.click();

  // File viewer should show content
  const fileViewer = page.getByTestId('file-viewer');
  await expect(fileViewer).toBeVisible();
  const viewerText = await fileViewer.innerText();
  expect(viewerText).toContain('Hello World!');
});

test('VFS: backend toggle and files panel are visible', async ({ page }) => {
  await waitForReady(page);

  const backendToggle = page.getByTestId('backend-toggle');
  await expect(backendToggle).toBeVisible();
  await expect(backendToggle).toContainText('Memory');

  const filesPanel = page.getByTestId('files-panel');
  await expect(filesPanel).toBeVisible();
});
