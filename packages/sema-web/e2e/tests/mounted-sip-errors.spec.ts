import { test, expect } from "@playwright/test";
import { waitForSema, semaEval, captureBrowserFailures } from "../helpers";

test("malformed SIP inside a mounted component keeps siblings interactive", async ({ page }) => {
  const failures = captureBrowserFailures(page);
  await page.goto("/mounted-sip-errors.html");
  await waitForSema(page);

  await expect(page.locator("#before")).toHaveText("before");
  await expect(page.locator("#middle")).toHaveText("ok");
  await expect(page.locator("#after")).toHaveText("after");

  await semaEval(page, '(put! mode "bad-tag")');
  await expect(page.locator("#before")).toHaveText("before");
  await expect(page.locator("#middle")).toHaveCount(0);
  await expect(page.locator("#after")).toHaveText("after");

  await page.click("#after");
  await expect(page.locator("#clicks")).toHaveText("1");
  expect(failures.consoleErrors.some((msg) => msg.includes("sip-render:invalid-tag:bad tag"))).toBe(true);
  expect(failures.pageErrors).toEqual([]);

  await semaEval(page, '(put! mode "ok")');
  await expect(page.locator("#middle")).toHaveText("ok");
});

test("malformed SIP attributes are skipped without disabling sibling attrs or events", async ({ page }) => {
  const failures = captureBrowserFailures(page);
  await page.goto("/mounted-sip-errors.html");
  await waitForSema(page);

  await semaEval(page, '(put! mode "bad-attr")');
  await expect(page.locator("#after")).toHaveText("after");
  await page.click("#after");
  await expect(page.locator("#clicks")).toHaveText("1");

  expect(failures.consoleErrors.some((msg) => msg.includes("sip-render:attribute:bad attr name"))).toBe(true);
  expect(failures.pageErrors).toEqual([]);
});
