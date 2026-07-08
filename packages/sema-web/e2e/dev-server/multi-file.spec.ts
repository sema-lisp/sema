import { test, expect } from "@playwright/test";
import { type ChildProcess } from "node:child_process";
import path from "node:path";
import fs from "node:fs";
import os from "node:os";
import { startDevServer } from "./helpers";

// A multi-file app (entry `(import "./util.sema")`) can't resolve modules
// against the browser's absent filesystem, so `sema web` compiles it to a .vfs
// on the fly and serves that. This asserts the whole path works with no extra
// steps — the imported module's export renders.
test.describe.configure({ mode: "serial" });

const PORT = 3046;
let server: ChildProcess | undefined;
let tmpDir: string;

test.beforeAll(async () => {
  tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "sema-web-mf-"));
  fs.writeFileSync(
    path.join(tmpDir, "util.sema"),
    `(module util (export greeting)\n  (define (greeting) "Imported module works"))\n`,
  );
  fs.writeFileSync(
    path.join(tmpDir, "app.sema"),
    `(import "./util.sema")\n(dom/set-text! (dom/query "#app") (greeting))\n`,
  );
  server = await startDevServer(path.join(tmpDir, "app.sema"), PORT);
});

test.afterAll(() => {
  server?.kill();
  if (tmpDir) fs.rmSync(tmpDir, { recursive: true, force: true });
});

test("runs a multi-file app (import resolves via the built .vfs)", async ({ page }) => {
  await page.goto(`http://127.0.0.1:${PORT}/`);
  await page.waitForFunction(() => (window as any).__semaInitialized === true, null, {
    timeout: 20_000,
  });
  const initError = await page.evaluate(() => (window as any).__semaInitError);
  expect(initError, "SemaWeb.init() should not error").toBeFalsy();

  // The imported module's export rendered — the `import` resolved in the browser.
  // #app / #__sema-error come from crates/sema/src/web/shell.html (the `sema web`
  // dev-server template), outside packages/sema-web — no data-testid to add here.
  await expect(page.locator("#app")).toHaveText("Imported module works");
  // No error overlay.
  await expect(page.locator("#__sema-error")).toHaveCount(0);
});
