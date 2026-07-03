import { test, expect } from "@playwright/test";
import { waitForSema, semaEval, captureConsoleErrors } from "../helpers";

test("component recovers after render error", async ({ page }) => {
  await page.goto("/error-recovery.html");
  const errors = captureConsoleErrors(page);
  await waitForSema(page);

  // Initially renders OK
  await expect(page.locator("#content")).toHaveText("OK");

  // Trigger an error in the render function
  await semaEval(page, "(put! should-error true)");

  // Wait a tick for the error to propagate
  await page.waitForTimeout(200);

  // Page should not have crashed (no unhandled rejection)
  // Console errors should have been captured
  expect(errors.length).toBeGreaterThan(0);

  // Recover by setting state back to false
  await semaEval(page, "(put! should-error false)");

  // Should render OK again
  await expect(page.locator("#content")).toHaveText("OK");
});
