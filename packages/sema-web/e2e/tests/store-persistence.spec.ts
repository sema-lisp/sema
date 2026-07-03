import { test, expect } from "@playwright/test";
import { waitForSema, semaEval } from "../helpers";

test("store persists values across page reloads", async ({ page }) => {
  await page.goto("/store.html");
  await waitForSema(page);

  // Set a value in the store
  await semaEval(page, '(store/set! "test-key" 42)');

  // Read it back
  const value = await semaEval(page, '(store/get "test-key")');
  expect(String(value?.value ?? value)).toContain("42");

  // Reload the page
  await page.reload();
  await waitForSema(page);

  // Value should persist
  const afterReload = await semaEval(page, '(store/get "test-key")');
  expect(String(afterReload?.value ?? afterReload)).toContain("42");
});
