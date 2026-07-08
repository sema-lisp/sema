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

    await expect(page.getByTestId("board-header")).toBeVisible();
    await expect(page.getByTestId("column-todo")).toBeVisible();
    await expect(page.getByTestId("column-in-progress")).toBeVisible();
    await expect(page.getByTestId("column-done")).toBeVisible();

    // Seed data: 3 todo, 2 in-progress, 1 done (see make-seed-data in board.sema)
    await expect(page.getByTestId("count-todo")).toHaveText("3");
    await expect(page.getByTestId("count-in-progress")).toHaveText("2");
    await expect(page.getByTestId("count-done")).toHaveText("1");

    await expect(page.getByTestId("progress-text")).toContainText("1/6");
    await expect(page.getByTestId("board-card")).toHaveCount(6);
  });

  test("manual card creation via the add-card form", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    await page.getByTestId("add-card-todo").click();
    await expect(page.getByTestId("add-card-form")).toBeVisible();

    await page.getByTestId("add-card-input").fill("Ship the release notes");
    await page.getByTestId("submit-add-card").click();

    // Form closes and the new card appears in the todo column
    await expect(page.getByTestId("add-card-form")).toHaveCount(0);
    await expect(page.getByTestId("count-todo")).toHaveText("4");
    await expect(
      page.getByTestId("column-todo").getByTestId("card-title").filter({
        hasText: "Ship the release notes",
      }),
    ).toBeVisible();
  });

  test("moving a card between columns via the move buttons", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    // "Implement user authentication" is seeded in the in-progress column
    const card = page.getByTestId("board-card").filter({
      hasText: "Implement user authentication",
    });
    await expect(card).toBeVisible();

    await card.getByTestId("move-right-btn").click();

    await expect(page.getByTestId("count-in-progress")).toHaveText("1");
    await expect(page.getByTestId("count-done")).toHaveText("2");
    await expect(
      page.getByTestId("column-done").getByTestId("board-card").filter({
        hasText: "Implement user authentication",
      }),
    ).toBeVisible();
  });

  test("card modal opens and supports priority cycling", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    const card = page.getByTestId("board-card").filter({ hasText: "Write API documentation" });
    await card.click();

    const modal = page.getByTestId("card-modal");
    await expect(modal).toBeVisible();
    await expect(page.getByTestId("modal-title")).toHaveText("Write API documentation");

    // Seeded priority is "medium" -> cycles to "high"
    await page.getByTestId("cycle-priority").click();
    await expect(modal.getByTestId("priority-badge")).toHaveText("high");

    await page.getByTestId("modal-close").click();
    await expect(modal).toHaveCount(0);
  });
});

test.describe("Board demo — AI task generation (mocked LLM stream)", () => {
  test("AI Generate streams and appends 3 AI-tagged cards", async ({ page }) => {
    await page.goto("/board.html");
    await waitForSema(page);

    await expect(page.getByTestId("board-card")).toHaveCount(6);

    await page.getByTestId("ai-generate-btn").click();

    // Button flips to a disabled "Generating..." state while the stream is open
    await expect(page.getByTestId("ai-generate-btn")).toHaveText("Generating...");

    // The mock proxy recognizes the board's task-generation prompt and streams
    // back a canned JSON array of 3 tasks; board.sema decodes it via its
    // poll-ai-stream interval and appends AI-tagged cards to the "todo" column.
    await expect(page.getByTestId("ai-badge")).toHaveCount(3, { timeout: 10_000 });
    await expect(page.getByTestId("board-card")).toHaveCount(9);

    // Button reverts once generation completes
    await expect(page.getByTestId("ai-generate-btn")).toHaveText("✨ AI Generate");

    const aiTitles = await page
      .getByTestId("board-card")
      .filter({ has: page.getByTestId("ai-badge") })
      .getByTestId("card-title")
      .allTextContents();
    expect(aiTitles).toEqual([
      "Set up staging environment",
      "Add integration test suite",
      "Write onboarding docs",
    ]);
  });
});
