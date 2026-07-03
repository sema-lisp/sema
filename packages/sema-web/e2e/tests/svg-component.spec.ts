import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

test("mounted SVG SIP uses real namespaces and delegated events in Chromium", async ({ page }) => {
  await page.goto("/svg-component.html");
  await waitForSema(page);

  const namespaces = await page.evaluate(() => {
    const icon = document.querySelector("#icon")!;
    const use = document.querySelector("#use-dot")!;
    const htmlInside = document.querySelector("#html-inside")!;
    return {
      icon: icon.namespaceURI,
      useHref: use.getAttributeNS("http://www.w3.org/1999/xlink", "href"),
      htmlInside: htmlInside.namespaceURI,
    };
  });

  expect(namespaces).toEqual({
    icon: "http://www.w3.org/2000/svg",
    useHref: "#dot",
    htmlInside: "http://www.w3.org/1999/xhtml",
  });

  await expect(page.locator("#toggle-state")).toHaveText("off");
  await page.click("#icon");
  await expect(page.locator("#toggle-state")).toHaveText("on");
});
