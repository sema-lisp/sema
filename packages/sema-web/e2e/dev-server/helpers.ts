import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import fs from "node:fs";

const dirname = path.dirname(fileURLToPath(import.meta.url));
/** Repo root, from packages/sema-web/e2e/dev-server/. */
export const repoRoot = path.resolve(dirname, "../../../..");

/** Resolve the built `sema` binary, preferring release over debug. */
export function semaBinary(): string {
  const release = path.join(repoRoot, "target", "release", "sema");
  const debug = path.join(repoRoot, "target", "debug", "sema");
  if (fs.existsSync(release)) return release;
  if (fs.existsSync(debug)) return debug;
  throw new Error("no `sema` binary found — run `make web-runtime` then `cargo build`");
}

/** Spawn `sema web <app> --port <port> --no-open` and wait until it serves. */
export async function startDevServer(app: string, port: number): Promise<ChildProcess> {
  const server = spawn(
    semaBinary(),
    ["web", app, "--port", String(port), "--no-open"],
    { cwd: repoRoot, stdio: "pipe" },
  );
  server.stderr?.on("data", (d) => process.stderr.write(`[sema web] ${d}`));
  await waitForHttp(`http://127.0.0.1:${port}/`, 25_000);
  return server;
}

async function waitForHttp(url: string, timeoutMs: number): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`dev server did not come up at ${url} within ${timeoutMs}ms`);
}
