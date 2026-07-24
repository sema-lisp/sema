import { devices, expect, test, type Locator, type Page } from '@playwright/test';

test.use({ ...devices['Pixel 7'] });

async function waitForReady(page: Page) {
  await page.goto('/');
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });
}

async function panVertically(locator: Locator, deltaY: number) {
  const { centerX, centerY } = await locator.evaluate((target: HTMLElement) => {
    const bounds = target.getBoundingClientRect();
    return {
      centerX: bounds.left + bounds.width / 2,
      centerY: bounds.top + bounds.height / 2,
    };
  });

  const start = [{ identifier: 0, clientX: centerX, clientY: centerY }];
  await locator.dispatchEvent('touchstart', {
    touches: start,
    changedTouches: start,
    targetTouches: start,
  });

  const end = [{ identifier: 0, clientX: centerX, clientY: centerY + deltaY }];
  await locator.dispatchEvent('touchmove', {
    touches: end,
    changedTouches: end,
    targetTouches: end,
  });
  await locator.dispatchEvent('touchend', {
    touches: [],
    changedTouches: end,
    targetTouches: [],
  });
}

async function blockSize(locator: Locator) {
  return locator.evaluate((element: HTMLElement) => element.getBoundingClientRect().height);
}

test('mobile splitters resize all stacked panes by touch and persist their sizes', async ({ page }) => {
  await waitForReady(page);

  const splitters = [
    page.getByTestId('splitter-sidebar'),
    page.getByTestId('splitter-editor'),
    page.getByTestId('splitter-output'),
    page.getByTestId('splitter-filetree'),
  ];

  for (const splitter of splitters) {
    await expect(splitter).toHaveAttribute('direction', 'vertical');
    await expect(splitter).toHaveAttribute('aria-orientation', 'horizontal');
    const bounds = await splitter.boundingBox();
    expect(bounds?.width).toBeGreaterThan(300);
    expect(bounds?.height).toBe(4);
  }

  const cases = [
    {
      splitter: splitters[0],
      pane: page.locator('.sidebar'),
      deltaY: 12,
      expectedDelta: 12,
      storageKey: 'mobileSidebarH',
    },
    {
      splitter: splitters[1],
      pane: page.locator('.editor-pane'),
      // Shrink the editor so the right column keeps its guaranteed minimum.
      deltaY: -12,
      expectedDelta: -12,
      storageKey: 'mobileEditorH',
    },
    {
      splitter: splitters[2],
      pane: page.locator('#files-body'),
      deltaY: -12,
      expectedDelta: 12,
      storageKey: 'mobileFilesH',
    },
    {
      splitter: splitters[3],
      pane: page.getByTestId('file-tree'),
      deltaY: 12,
      expectedDelta: 12,
      storageKey: 'mobileFiletreeH',
    },
  ];

  const persisted = new Map<string, number>();
  for (const resizeCase of cases) {
    const before = await blockSize(resizeCase.pane);
    await panVertically(resizeCase.splitter, resizeCase.deltaY);
    const after = await blockSize(resizeCase.pane);

    expect(after).toBeCloseTo(before + resizeCase.expectedDelta, 0);
    await expect(resizeCase.splitter).toHaveAttribute('aria-valuenow', String(Math.round(after)));

    const saved = await page.evaluate((key) => {
      const state = JSON.parse(localStorage.getItem('sema-playground') ?? '{}');
      return state[key];
    }, resizeCase.storageKey);
    expect(saved).toBeCloseTo(after, 0);
    persisted.set(resizeCase.storageKey, saved);
  }

  await page.reload();
  await expect(page.getByTestId('status')).toHaveClass(/status-ready/, { timeout: 15000 });

  for (const resizeCase of cases) {
    await expect.poll(() => blockSize(resizeCase.pane)).toBeCloseTo(
      persisted.get(resizeCase.storageKey)!,
      0,
    );
  }
});
