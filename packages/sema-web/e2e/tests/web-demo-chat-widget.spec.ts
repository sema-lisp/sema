import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

// Covers examples/web-demo/chat-widget.sema (embeddable Intercom-style chat
// widget). The fixture at fixtures/scripts/chat-widget.sema is a snapshot
// copy of the demo.
//
// Same mocked SSE proxy as web-demo-chat.spec.ts (see e2e/mock-proxy.ts):
// deterministic `Mock reply to: <user text>` streamed word-by-word.

test.describe("Chat widget demo — mocked SSE streaming", () => {
  test("fab toggles the panel open and closed", async ({ page }) => {
    await page.goto("/chat-widget.html");
    await waitForSema(page);

    await expect(page.getByTestId("chat-fab")).toBeVisible();
    await expect(page.getByTestId("chat-panel")).toHaveCount(0);

    await page.getByTestId("chat-fab").click();
    await expect(page.getByTestId("chat-panel")).toBeVisible();

    await page.getByTestId("close-btn").click();
    await expect(page.getByTestId("chat-panel")).toHaveCount(0);
  });

  test("sending a message shows a typing indicator, streams incrementally, then lands in the transcript", async ({ page }) => {
    await page.goto("/chat-widget.html");
    await waitForSema(page);

    await page.getByTestId("chat-fab").click();
    await expect(page.getByTestId("chat-panel")).toBeVisible();

    // Track the streaming bubble's shape over time: it starts as a
    // typing-indicator (no text yet), then flips to a growing streaming-msg
    // once the first token arrives.
    // NOTE: runs inside the page's own JS context via page.evaluate(), so it
    // uses document.querySelector directly rather than a Playwright locator.
    await page.evaluate(() => {
      (window as any).__states = [];
      const container = document.querySelector("#widget-msg-list")!;
      const obs = new MutationObserver(() => {
        const states = (window as any).__states as string[];
        const typing = document.querySelector('[data-testid="typing-indicator"]');
        const streaming = document.querySelector('[data-testid="streaming-msg"]');
        let marker: string | null = null;
        if (typing) marker = "TYPING";
        else if (streaming) marker = `TEXT:${streaming.textContent ?? ""}`;
        if (marker && states[states.length - 1] !== marker) states.push(marker);
      });
      obs.observe(container, { childList: true, subtree: true, characterData: true });
    });

    await page.getByTestId("chat-input").fill("Hi widget");
    await page.getByTestId("send-btn").click();

    await expect(page.getByTestId("msg-user")).toHaveText("Hi widget");
    await expect(page.getByTestId("chat-input")).toHaveValue("");

    const finalText = "Mock reply to: Hi widget";

    await expect(page.getByTestId("msg-assistant")).toHaveText(finalText, {
      timeout: 5_000,
    });
    await expect(page.getByTestId("streaming-msg")).toHaveCount(0);
    await expect(page.getByTestId("typing-indicator")).toHaveCount(0);

    const states: string[] = await page.evaluate(() => (window as any).__states);
    // Must have seen the typing indicator before any streamed text appeared.
    expect(states[0]).toBe("TYPING");
    const textStates = states.filter((s) => s.startsWith("TEXT:")).map((s) => s.slice(5));
    expect(textStates.length).toBeGreaterThan(1);
    expect(textStates[textStates.length - 1]).toBe(finalText);
    for (let i = 1; i < textStates.length; i++) {
      expect(textStates[i].length).toBeGreaterThan(textStates[i - 1].length);
    }
  });

  test("messages persist across reload via localStorage", async ({ page }) => {
    await page.goto("/chat-widget.html");
    await waitForSema(page);

    await page.getByTestId("chat-fab").click();
    await page.getByTestId("chat-input").fill("Remember this");
    await page.getByTestId("send-btn").click();

    await expect(page.getByTestId("msg-assistant")).toHaveText(
      "Mock reply to: Remember this",
      { timeout: 5_000 },
    );

    await page.reload();
    await waitForSema(page);
    await page.getByTestId("chat-fab").click();

    await expect(page.getByTestId("msg-user")).toHaveText("Remember this");
    await expect(page.getByTestId("msg-assistant")).toHaveText(
      "Mock reply to: Remember this",
    );
  });
});
