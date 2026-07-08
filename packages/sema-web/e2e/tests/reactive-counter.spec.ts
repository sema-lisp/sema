import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

test("reactive counter increments, decrements, and resets", async ({ page }) => {
  await page.goto("/counter.html");
  await waitForSema(page);

  const display = page.getByTestId("count-display");
  await expect(display).toHaveText("0");

  // Increment three times
  await page.getByTestId("btn-inc").click();
  await page.getByTestId("btn-inc").click();
  await page.getByTestId("btn-inc").click();
  await expect(display).toHaveText("3");

  // Decrement once
  await page.getByTestId("btn-dec").click();
  await expect(display).toHaveText("2");

  // Reset
  await page.getByTestId("btn-reset").click();
  await expect(display).toHaveText("0");
});
