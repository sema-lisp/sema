import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

test("autoLoad: false prevents script tags from executing", async ({ page }) => {
  await page.goto("/no-autoload.html");
  await waitForSema(page);

  // The inline sema script should NOT have run
  await expect(page.locator("#app")).toHaveText("untouched");
});
