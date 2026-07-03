import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

test("inline sema script sets text content", async ({ page }) => {
  await page.goto("/basic.html");
  await waitForSema(page);
  await expect(page.locator("#app")).toHaveText("Hello from Sema!");
});
