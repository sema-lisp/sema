import { test, expect, Page } from '@playwright/test';

async function waitForReady(page: Page) {
  await page.goto('/');
  await page.waitForSelector('#status.status-ready', { timeout: 15000 });
}

async function clickRun(page: Page) {
  await page.getByTestId('run-btn').click();
  await page.waitForSelector('#status.status-ready', { timeout: 30000 });
}

test('VFS: run file-creating script and check file tree', async ({ page }) => {
  await waitForReady(page);

  await page.getByTestId('editor').fill(`
(file/mkdir "test-dir")
(file/write "test-dir/hello.txt" "Hello from VFS!")
(println "done")
`);

  await clickRun(page);

  // File tree is always visible in the files panel. It dogfoods <sema-tree>,
  // so entries are <sema-tree-item> nodes keyed by their `label` attribute.
  const fileTree = page.getByTestId('file-tree');
  await expect(fileTree).toBeVisible();
  await expect(fileTree.locator('sema-tree-item[label="test-dir"]')).toBeVisible();
  await expect(fileTree.locator('sema-tree-item[label="hello.txt"]')).toBeVisible();
});

test('VFS: clicking a file shows content in preview pane', async ({ page }) => {
  await waitForReady(page);

  await page.getByTestId('editor').fill(`
(file/write "greeting.txt" "Hello World!")
`);
  await clickRun(page);

  // Click on the file in the tree (a <sema-tree-item> leaf)
  const fileEntry = page.locator('sema-tree-item[label="greeting.txt"]');
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
