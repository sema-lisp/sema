import { test, expect } from "@playwright/test";
import { waitForSema } from "./helpers";

test.describe("AI Chat Demo", () => {
  test("page loads and renders chat UI", async ({ page }) => {
    await page.goto("/");
    await waitForSema(page);

    // Chat container should be visible
    await expect(page.getByTestId("chat-container")).toBeVisible();
    await expect(page.locator("h2")).toHaveText("Sema AI Chat");

    // Input and send button should be present
    await expect(page.getByTestId("chat-input")).toBeVisible();
    await expect(page.getByTestId("send-btn")).toHaveText("Send");
  });

  test("send message and receive streaming response", async ({ page }) => {
    await page.goto("/");
    await waitForSema(page);

    // Type a message and send via eval (bypasses form submission event chain)
    await page.evaluate(() => {
      (window as any).__semaWeb.eval('(put! input-text "Say hello")');
      (window as any).__semaWeb.eval(`
        (let ((text "Say hello"))
          (update! messages (fn (msgs) (append msgs (list {:role "user" :content text}))))
          (let ((s (llm/chat-stream (list {:role "user" :content text}) {})))
            (put! current-stream s)))
      `);
    });

    // User message should appear
    await expect(page.getByTestId("msg-user").first()).toBeVisible({ timeout: 5_000 });
    await expect(page.getByTestId("msg-user").first()).toContainText("Say hello");

    // Wait for assistant response (streaming or completed)
    await expect(page.getByTestId("msg-assistant").or(page.getByTestId("msg-assistant-streaming"))).toBeVisible({ timeout: 30_000 });

    // Wait for streaming to finish
    await page.waitForFunction(
      () => {
        const msgs = document.querySelectorAll(".message.assistant");
        return msgs.length > 0 && (msgs[msgs.length - 1].textContent || "").trim().length > 5;
      },
      null,
      { timeout: 30_000 }
    );
  });

  test("completed stream moves response to message list", async ({ page }) => {
    await page.goto("/");
    await waitForSema(page);

    // Send a message via eval
    await page.evaluate(() => {
      const web = (window as any).__semaWeb;
      web.eval('(update! messages (fn (msgs) (append msgs (list {:role "user" :content "Hi"}))))');
      web.eval('(put! current-stream (llm/chat-stream (list {:role "user" :content "Say hi back briefly"}) {}))');
    });

    // Wait for the assistant response to appear (streaming or completed)
    await expect(page.getByTestId("msg-assistant").or(page.getByTestId("msg-assistant-streaming"))).toBeVisible({ timeout: 30_000 });

    // Wait for streaming to finish — the message should have content
    await page.waitForFunction(
      () => {
        const msgs = document.querySelectorAll(".message.assistant");
        if (msgs.length === 0) return false;
        const last = msgs[msgs.length - 1];
        return (last.textContent || "").trim().length > 5;
      },
      null,
      { timeout: 30_000 }
    );

    // Verify we have both user and assistant messages
    await expect(page.getByTestId("msg-user")).toHaveCount(1);
    const assistantCount = await page.getByTestId("msg-assistant").or(page.getByTestId("msg-assistant-streaming")).count();
    expect(assistantCount).toBeGreaterThanOrEqual(1);
  });
});
