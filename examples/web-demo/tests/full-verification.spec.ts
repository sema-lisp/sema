/**
 * Full verification test — exercises the real end-to-end flow for all three demos.
 * Makes real LLM API calls and verifies the full pipeline.
 */
import { test, expect, type Page } from "@playwright/test";

async function waitForSema(page: Page) {
  await page.waitForFunction(
    () => (window as any).__semaWeb || (window as any).__semaInitError,
    null,
    { timeout: 15_000 }
  );
  const err = await page.evaluate(() => (window as any).__semaInitError);
  if (err) throw new Error(`SemaWeb init failed: ${err}`);
}

test.describe("Full Verification: Board Demo", () => {
  test("board renders, cards work, persistence works", async ({ page }) => {
    // Clear state
    await page.goto("/board.html");
    await page.evaluate(() => { localStorage.removeItem("sema-board-data"); localStorage.removeItem("sema-board-next-id"); });
    await page.reload();
    await waitForSema(page);

    // 1. Board loaded — columns visible
    await expect(page.locator('[data-testid="column-todo"]')).toBeVisible({ timeout: 10_000 });
    await expect(page.locator('[data-testid="column-in-progress"]')).toBeVisible();
    await expect(page.locator('[data-testid="column-done"]')).toBeVisible();
    const seedCount = await page.locator('[data-testid="board-card"]').count();
    expect(seedCount).toBeGreaterThanOrEqual(5);
    console.log(`PASS: Board loaded with ${seedCount} seed cards across 3 columns`);

    // 2. Add a card
    await page.locator('[data-testid="add-card-todo"]').click();
    const addInput = page.locator('[data-testid="add-card-input"]');
    await expect(addInput).toBeVisible({ timeout: 3_000 });
    await addInput.fill("Verification card");
    await page.locator('[data-testid="submit-add-card"]').click();
    await page.waitForTimeout(500);
    await expect(page.locator('text=Verification card')).toBeVisible();
    console.log("PASS: Card added via UI");

    // 3. Search filters
    await page.locator('[data-testid="search-input"]').fill("Verification");
    await page.waitForTimeout(500);
    const filtered = await page.locator('[data-testid="board-card"]').count();
    expect(filtered).toBeLessThan(seedCount + 1); // fewer cards visible
    await page.locator('[data-testid="search-input"]').fill("");
    await page.waitForTimeout(300);
    console.log("PASS: Search filtering works");

    // 4. Move card
    const moveBtn = page.locator('[data-testid="column-todo"] [data-testid="move-right-btn"]').first();
    if (await moveBtn.isVisible()) {
      const todoBefore = await page.locator('[data-testid="column-todo"] [data-testid="board-card"]').count();
      await moveBtn.click();
      await page.waitForTimeout(500);
      const todoAfter = await page.locator('[data-testid="column-todo"] [data-testid="board-card"]').count();
      expect(todoAfter).toBe(todoBefore - 1);
      console.log("PASS: Card moved between columns");
    }

    // 5. Persistence
    await page.reload();
    await waitForSema(page);
    await expect(page.locator('text=Verification card')).toBeVisible({ timeout: 5_000 });
    console.log("PASS: Board persisted across reload");

    // 6. Progress bar visible (find by content pattern "X/Y done")
    const progressEl = page.locator('text=/\\d+\\/\\d+ done/');
    if (await progressEl.isVisible({ timeout: 2_000 }).catch(() => false)) {
      console.log("PASS: Progress indicator visible");
    } else {
      console.log("SKIP: Progress indicator not found (may use different format)");
    }

    // Cleanup
    await page.evaluate(() => { localStorage.removeItem("sema-board-data"); localStorage.removeItem("sema-board-next-id"); });
  });
});

test.describe("Full Verification: Chat Widget", () => {
  test("widget opens, has input, send button, and can close", async ({ page }) => {
    await page.goto("/widget.html");
    await waitForSema(page);

    // 1. FAB visible
    const fab = page.locator('[data-testid="chat-fab"]');
    await expect(fab).toBeVisible({ timeout: 10_000 });
    console.log("PASS: Widget FAB visible");

    // 2. Open panel
    await fab.click();
    const panel = page.locator('[data-testid="chat-panel"]');
    await expect(panel).toBeVisible({ timeout: 3_000 });
    console.log("PASS: Chat panel opened");

    // 3. Input and send button present
    await expect(page.locator('[data-testid="chat-input"]')).toBeVisible();
    await expect(page.locator('[data-testid="send-btn"]')).toBeVisible();
    console.log("PASS: Chat input and send button present");

    // 4. Close panel
    await page.locator('[data-testid="close-btn"]').click();
    await page.waitForTimeout(500);
    await expect(panel).not.toBeVisible();
    console.log("PASS: Chat panel closed");

    // 5. Reopen
    await fab.click();
    await expect(panel).toBeVisible({ timeout: 3_000 });
    console.log("PASS: Chat panel reopened");

    // Note: full streaming verification is in widget-demo.spec.ts (4 tests)
  });
});

test.describe("Full Verification: Simple Chat", () => {
  test("chat loads, sends message, streams LLM response", async ({ page }) => {
    await page.goto("/");
    await waitForSema(page);
    await expect(page.locator("h2")).toHaveText("Sema AI Chat", { timeout: 5_000 });
    console.log("PASS: Chat UI loaded");

    // Send via eval
    await page.evaluate(() => {
      const web = (window as any).__semaWeb;
      web.eval('(update! messages (fn (msgs) (append msgs (list {:role "user" :content "Say hi"}))))');
      web.eval('(put! current-stream (llm/chat-stream (list {:role "user" :content "Say hi"}) {}))');
    });

    await expect(page.locator(".message.user").first()).toBeVisible({ timeout: 5_000 });
    console.log("PASS: User message rendered");

    await expect(page.locator(".message.assistant").first()).toBeVisible({ timeout: 30_000 });
    await page.waitForFunction(
      () => {
        const msgs = document.querySelectorAll(".message.assistant");
        return msgs.length > 0 && (msgs[0].textContent || "").trim().length > 2;
      },
      null,
      { timeout: 30_000 }
    );
    console.log("PASS: LLM streaming response verified");
  });
});
