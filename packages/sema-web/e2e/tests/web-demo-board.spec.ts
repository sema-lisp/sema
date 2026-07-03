import { test, expect } from "@playwright/test";
import { waitForSema } from "../helpers";

// Covers examples/web-demo/board.sema (Trello-style AI project board).
// The fixture at fixtures/scripts/board.sema is a snapshot copy — the demo
// source itself is off-limits (owned by a concurrent modernization pass).
//
// LLM calls go through llm/chat-stream -> SSE POST http://localhost:3002/stream,
// served here by the mock proxy (e2e/mock-proxy.ts) started as part of the
// Playwright webServer config. The mock proxy is stateless: it derives its
// reply deterministically from the request body, so parallel spec files
// hitting the same shared server don't race each other.

test.describe("Board demo — core board rendering and manual interactions", () => {
  test("initial board renders seed data across columns", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    await expect(page.locator('[data-testid="board-header"]')).toBeVisible();
    await expect(page.locator('[data-testid="column-todo"]')).toBeVisible();
    await expect(page.locator('[data-testid="column-in-progress"]')).toBeVisible();
    await expect(page.locator('[data-testid="column-done"]')).toBeVisible();

    // Seed data: 3 todo, 2 in-progress, 1 done (see make-seed-data in board.sema)
    await expect(page.locator('[data-testid="count-todo"]')).toHaveText("3");
    await expect(page.locator('[data-testid="count-in-progress"]')).toHaveText("2");
    await expect(page.locator('[data-testid="count-done"]')).toHaveText("1");

    await expect(page.locator('[data-testid="progress-text"]')).toContainText("1/6");
    await expect(page.locator('[data-testid="board-card"]')).toHaveCount(6);
  });

  test("manual card creation via the add-card form", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    await page.click('[data-testid="add-card-todo"]');
    await expect(page.locator('[data-testid="add-card-form"]')).toBeVisible();

    await page.fill('[data-testid="add-card-input"]', "Ship the release notes");
    await page.click('[data-testid="submit-add-card"]');

    // Form closes and the new card appears in the todo column
    await expect(page.locator('[data-testid="add-card-form"]')).toHaveCount(0);
    await expect(page.locator('[data-testid="count-todo"]')).toHaveText("4");
    await expect(
      page.locator('[data-testid="column-todo"] [data-testid="card-title"]', {
        hasText: "Ship the release notes",
      }),
    ).toBeVisible();
  });

  test("moving a card between columns via the move buttons", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    // "Implement user authentication" is seeded in the in-progress column
    const card = page.locator('[data-testid="board-card"]', {
      hasText: "Implement user authentication",
    });
    await expect(card).toBeVisible();

    await card.locator('[data-testid="move-right-btn"]').click();

    await expect(page.locator('[data-testid="count-in-progress"]')).toHaveText("1");
    await expect(page.locator('[data-testid="count-done"]')).toHaveText("2");
    await expect(
      page.locator('[data-testid="column-done"] [data-testid="board-card"]', {
        hasText: "Implement user authentication",
      }),
    ).toBeVisible();
  });

  test("card modal opens and supports priority cycling", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    const card = page.locator('[data-testid="board-card"]', { hasText: "Write API documentation" });
    await card.click();

    const modal = page.locator('[data-testid="card-modal"]');
    await expect(modal).toBeVisible();
    await expect(page.locator('[data-testid="modal-title"]')).toHaveText("Write API documentation");

    // Seeded priority is "medium" -> cycles to "high"
    await page.click('[data-testid="cycle-priority"]');
    await expect(modal.locator('[data-testid="priority-badge"]')).toHaveText("high");

    await page.click('[data-testid="modal-close"]');
    await expect(modal).toHaveCount(0);
  });
});

test.describe("Board demo — AI task generation (mocked LLM stream)", () => {
  test("AI Generate streams and appends 3 AI-tagged cards", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    await expect(page.locator('[data-testid="board-card"]')).toHaveCount(6);

    await page.click('[data-testid="ai-generate-btn"]');

    // Button flips to a disabled "Generating..." state while the stream is open
    await expect(page.locator('[data-testid="ai-generate-btn"]')).toHaveText("Generating...");

    // The mock proxy recognizes the board's task-generation prompt and streams
    // back a canned JSON array of 3 tasks; board.sema decodes it via its
    // poll-ai-stream interval and appends AI-tagged cards to the "todo" column.
    await expect(page.locator('[data-testid="ai-badge"]')).toHaveCount(3, { timeout: 10_000 });
    await expect(page.locator('[data-testid="board-card"]')).toHaveCount(9);

    // Button reverts once generation completes
    await expect(page.locator('[data-testid="ai-generate-btn"]')).toHaveText("✨ AI Generate");

    const aiTitles = await page
      .locator('[data-testid="board-card"]', { has: page.locator('[data-testid="ai-badge"]') })
      .locator('[data-testid="card-title"]')
      .allTextContents();
    expect(aiTitles).toEqual([
      "Set up staging environment",
      "Add integration test suite",
      "Write onboarding docs",
    ]);
  });
});
