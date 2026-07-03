import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

test("reactive counter increments, decrements, and resets", async ({ page }) => {
  await page.goto("/counter.html");
  await waitForSema(page);

  const display = page.locator("#count-display");
  await expect(display).toHaveText("0");

  // Increment three times
  await page.click("#btn-inc");
  await page.click("#btn-inc");
  await page.click("#btn-inc");
  await expect(display).toHaveText("3");

  // Decrement once
  await page.click("#btn-dec");
  await expect(display).toHaveText("2");

  // Reset
  await page.click("#btn-reset");
  await expect(display).toHaveText("0");
});
