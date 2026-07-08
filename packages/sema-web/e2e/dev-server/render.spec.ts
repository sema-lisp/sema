import { test, expect } from "@playwright/test";
import { type ChildProcess } from "node:child_process";
import path from "node:path";
import fs from "node:fs";
import os from "node:os";
import { repoRoot, startDevServer } from "./helpers";

// Boot the real `sema web` binary against a temp copy of an example app and
// assert the browser actually renders + runs it (WASM VM loads, source evals,
// DOM + event handlers work). This is the capstone gate for "serve + render".
test.describe.configure({ mode: "serial" });

const PORT = 3044;
let server: ChildProcess | undefined;
let tmpDir: string;

test.beforeAll(async () => {
  tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "sema-web-e2e-"));
  const src = fs.readFileSync(path.join(repoRoot, "examples/web/counter.sema"), "utf8");
  fs.writeFileSync(path.join(tmpDir, "app.sema"), src);
  server = await startDevServer(path.join(tmpDir, "app.sema"), PORT);
});

test.afterAll(() => {
  server?.kill();
  if (tmpDir) fs.rmSync(tmpDir, { recursive: true, force: true });
});

test("renders and runs the app in the browser", async ({ page }) => {
  await page.goto(`http://127.0.0.1:${PORT}/`);

  // SemaWeb.init() must succeed (WASM VM + runtime loaded via the import map).
  await page.waitForFunction(() => (window as any).__semaInitialized === true, null, {
    timeout: 20_000,
  });
  const initError = await page.evaluate(() => (window as any).__semaInitError);
  expect(initError, "SemaWeb.init() should not error").toBeFalsy();

  // The app's Sema source rendered its DOM.
  await expect(page.getByRole("heading", { name: "Sema Counter" })).toBeVisible();
  // #app comes from crates/sema/src/web/shell.html (the `sema web` dev-server
  // template); the app source itself is examples/web/counter.sema — both are
  // outside packages/sema-web, so there's no data-testid to add here.
  await expect(page.locator("#app")).toContainText("0");

  // Event handlers work end-to-end: clicking + runs Sema, updates the DOM.
  await page.getByRole("button", { name: "+" }).click();
  await expect(page.locator("#app")).toContainText("1");
});

test("hot-reloads the browser when the app source changes", async ({ page }) => {
  await page.goto(`http://127.0.0.1:${PORT}/`);
  await page.waitForFunction(() => (window as any).__semaInitialized === true, null, {
    timeout: 20_000,
  });
  await expect(page.getByRole("heading", { name: "Sema Counter" })).toBeVisible();

  // Edit the served source; the dev server's watcher + poll loop should trigger
  // a full reload that fetches the (cache-busted) new source.
  const appPath = path.join(tmpDir, "app.sema");
  const edited = fs
    .readFileSync(appPath, "utf8")
    .replace("Sema Counter", "Reloaded Heading");
  fs.writeFileSync(appPath, edited);

  await expect(page.getByRole("heading", { name: "Reloaded Heading" })).toBeVisible({
    timeout: 15_000,
  });
});
