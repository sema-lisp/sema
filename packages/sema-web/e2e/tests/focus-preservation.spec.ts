import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

test("input retains focus and value after unrelated state change", async ({ page }) => {
  await page.goto("/focus.html");
  await waitForSema(page);

  const input = page.locator("#text-input");
  await input.click();
  await input.fill("hello");

  // Trigger a re-render via a different state atom (keyboard shortcut to avoid moving focus)
  // Use page.evaluate to call put! directly without clicking a button
  await page.evaluate(() => {
    (window as any).__semaWeb.eval("(update! counter (fn (n) (+ n 1)))");
  });

  // Wait for morphdom to patch
  await page.waitForTimeout(100);

  // Input should still be focused (no click moved focus away)
  await expect(input).toBeFocused();

  // Input value should be preserved by morphdom (not reset to state value)
  await expect(input).toHaveValue("hello");

  // Counter should have updated
  await expect(page.locator("#counter-display")).toHaveText("1");
});
