import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

// Covers examples/web-demo/chat.sema (AI chat widget with SSE-style streaming).
// The fixture at fixtures/scripts/chat.sema is a snapshot copy of the demo.
//
// The mock proxy (e2e/mock-proxy.ts) streams back
// `Mock reply to: <user text>` word-by-word over real SSE chunks (30ms apart),
// which lets us assert the UI renders the reply incrementally before it
// settles into the final transcript entry.

test.describe("Chat demo — mocked SSE streaming", () => {
  test("renders the chat UI", async ({ page }) => {
    await page.goto("/chat.html");
    await waitForSema(page);

    await expect(page.getByTestId("chat-container")).toBeVisible();
    await expect(page.getByTestId("chat-title")).toHaveText("Sema AI Chat");
    await expect(page.getByTestId("chat-input")).toBeVisible();
    await expect(page.getByTestId("send-btn")).toHaveText("Send");
  });

  test("sending a message streams the mocked reply incrementally, then lands in the transcript", async ({ page }) => {
    await page.goto("/chat.html");
    await waitForSema(page);

    // Record every distinct textContent the streaming bubble passes through
    // via MutationObserver — this observes every DOM patch morphdom makes,
    // so it can't miss an intermediate frame the way a polled assertion could.
    // NOTE: runs inside the page's own JS context via page.evaluate(), so it
    // uses document.querySelector directly rather than a Playwright locator.
    await page.evaluate(() => {
      (window as any).__streamStates = [];
      const container = document.querySelector("#message-list")!;
      const obs = new MutationObserver(() => {
        const el = document.querySelector('[data-testid="streaming-msg"]');
        if (el) {
          const states = (window as any).__streamStates as string[];
          const text = el.textContent ?? "";
          if (states[states.length - 1] !== text) states.push(text);
        }
      });
      obs.observe(container, { childList: true, subtree: true, characterData: true });
    });

    await page.getByTestId("chat-input").fill("Hello there");
    await page.getByTestId("send-btn").click();

    // User message appears immediately, input clears
    await expect(page.getByTestId("msg-user")).toHaveText("user: Hello there");
    await expect(page.getByTestId("chat-input")).toHaveValue("");

    const finalText = "Mock reply to: Hello there";

    // Eventually the stream completes and the reply moves into the static
    // transcript as a plain (non-streaming) assistant message.
    await expect(page.getByTestId("msg-assistant")).toHaveText(`assistant: ${finalText}`, {
      timeout: 5_000,
    });
    await expect(page.getByTestId("streaming-msg")).toHaveCount(0);
    await expect(page.getByTestId("msg-user")).toHaveCount(1);
    await expect(page.getByTestId("msg-assistant")).toHaveCount(1);

    // The observer must have recorded multiple distinct, growing text states —
    // proof the bubble filled in token-by-token rather than appearing whole.
    const states: string[] = await page.evaluate(() => (window as any).__streamStates);
    expect(states.length).toBeGreaterThan(1);
    expect(states[0]).toBe("assistant: ");
    expect(states[states.length - 1]).toBe(`assistant: ${finalText}`);
    for (let i = 1; i < states.length; i++) {
      expect(states[i].length).toBeGreaterThan(states[i - 1].length);
    }
  });
});
