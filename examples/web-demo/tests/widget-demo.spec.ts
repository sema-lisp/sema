import { test, expect, type Page } from "@playwright/test";

async function waitForSema(page: Page) {
  await page.waitForFunction(
    () => {
      const w = window as any;
      return w.__semaWeb || w.__semaInitError;
    },
    null,
    { timeout: 15_000 }
  );
  const initError = await page.evaluate(() => (window as any).__semaInitError);
  if (initError) throw new Error(`SemaWeb init failed: ${initError}`);
}

test.describe("Chat Widget Demo", () => {
  test("floating button is visible on page load", async ({ page }) => {
    await page.goto("/widget.html");
    await waitForSema(page);

    // FAB button should be visible
    const fab = page.locator('[data-testid="chat-fab"]');
    await expect(fab).toBeVisible({ timeout: 10_000 });
  });

  test("clicking FAB opens and closes the chat panel", async ({ page }) => {
    await page.goto("/widget.html");
    await waitForSema(page);

    const fab = page.locator('[data-testid="chat-fab"]');
    await expect(fab).toBeVisible({ timeout: 10_000 });

    // Panel should not be visible initially
    await expect(page.locator('[data-testid="chat-panel"]')).not.toBeVisible();

    // Click to open
    await fab.click();
    await expect(page.locator('[data-testid="chat-panel"]')).toBeVisible({ timeout: 5_000 });

    // Close via close button
    await page.locator('[data-testid="close-btn"]').click();
    await expect(page.locator('[data-testid="chat-panel"]')).not.toBeVisible({ timeout: 5_000 });
  });

  test("send a message and receive streaming response", async ({ page }) => {
    await page.goto("/widget.html");
    await waitForSema(page);

    // Open the widget
    await page.locator('[data-testid="chat-fab"]').click();
    await expect(page.locator('[data-testid="chat-panel"]')).toBeVisible({ timeout: 5_000 });

    // Type and send a message
    const input = page.locator('[data-testid="chat-input"]');
    await input.fill("Hello there");

    // Update reactive state to match the filled value
    await page.evaluate(() => {
      (window as any).__semaWeb.eval('(put! input-text "Hello there")');
    });

    await page.locator('[data-testid="send-btn"]').click();

    // User message should appear
    await expect(page.locator('[data-testid="msg-user"]').first()).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('[data-testid="msg-user"]').first()).toContainText("Hello there");

    // Wait for streaming to start (typing indicator or streaming text)
    await expect(
      page.locator('[data-testid="typing-indicator"], [data-testid="streaming-msg"]').first()
    ).toBeVisible({ timeout: 10_000 });

    // Wait for assistant response to complete
    await expect(page.locator('[data-testid="msg-assistant"]').first()).toBeVisible({ timeout: 30_000 });
  });

  test("conversation persists across reloads via localStorage", async ({ page }) => {
    await page.goto("/widget.html");
    await waitForSema(page);

    // Inject a message directly via eval to avoid needing the LLM proxy
    await page.evaluate(() => {
      const web = (window as any).__semaWeb;
      web.eval(`
        (begin
          (update! messages (fn (msgs)
            (append msgs (list {:role "user" :content "Persisted message"}
                               {:role "assistant" :content "I remember you"}))))
          (save-messages))
      `);
    });

    // Verify messages are visible
    await page.locator('[data-testid="chat-fab"]').click();
    await expect(page.locator('[data-testid="chat-panel"]')).toBeVisible({ timeout: 5_000 });
    await expect(page.locator('[data-testid="msg-user"]').first()).toContainText("Persisted message");
    await expect(page.locator('[data-testid="msg-assistant"]').first()).toContainText("I remember you");

    // Reload the page
    await page.reload();
    await waitForSema(page);

    // Open widget again
    await page.locator('[data-testid="chat-fab"]').click();
    await expect(page.locator('[data-testid="chat-panel"]')).toBeVisible({ timeout: 5_000 });

    // Messages should still be there from localStorage
    await expect(page.locator('[data-testid="msg-user"]').first()).toContainText("Persisted message");
    await expect(page.locator('[data-testid="msg-assistant"]').first()).toContainText("I remember you");
  });
});
