import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

test("multiple SemaWeb instances are isolated", async ({ page }) => {
  await page.goto("/multi-instance.html");
  await waitForSema(page);

  // Set up a counter in instance A
  await page.evaluate(() => {
    const webA = (window as any).__semaWebA;
    webA.eval('(def count-a (state 0))');
    webA.eval(`
      (define (counter-a-view)
        [:p {:id "count-a" :data-testid "count-a"} (number->string @count-a)])
    `);
    webA.eval('(mount! "#app-a" "counter-a-view")');
  });

  // Set up a counter in instance B
  await page.evaluate(() => {
    const webB = (window as any).__semaWebB;
    webB.eval('(def count-b (state 0))');
    webB.eval(`
      (define (counter-b-view)
        [:p {:id "count-b" :data-testid "count-b"} (number->string @count-b)])
    `);
    webB.eval('(mount! "#app-b" "counter-b-view")');
  });

  // Both start at 0
  await expect(page.getByTestId("count-a")).toHaveText("0");
  await expect(page.getByTestId("count-b")).toHaveText("0");

  // Increment only instance A
  await page.evaluate(() => {
    (window as any).__semaWebA.eval("(update! count-a (fn (n) (+ n 1)))");
  });

  // A shows 1, B still shows 0
  await expect(page.getByTestId("count-a")).toHaveText("1");
  await expect(page.getByTestId("count-b")).toHaveText("0");
});
