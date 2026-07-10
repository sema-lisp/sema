import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: '*.spec.ts',
  timeout: 30000,
  retries: 0,
  workers: 1,
  use: {
    baseURL: 'http://127.0.0.1:18888',
    // Local runs can set PW_CHANNEL=chrome to drive the system Chrome install
    // when the Playwright-managed chromium download stalls on this machine.
    ...(process.env.PW_CHANNEL ? { channel: process.env.PW_CHANNEL } : {}),
  },
  webServer: {
    // --bin sema: the workspace also builds a `sema-docs` binary, so a bare
    // `cargo run` is ambiguous ("could not determine which binary to run").
    // Serve a copy of the fixture, not the git-tracked original: the suite
    // saves through to the notebook file it serves, so runs are non-idempotent
    // and would otherwise mutate (and compound failures onto) the tracked demo.
    command:
      'mkdir -p target && cp examples/notebook/demo.sema-nb target/e2e-demo.sema-nb && cargo run --bin sema -- notebook serve -p 18888 target/e2e-demo.sema-nb',
    port: 18888,
    cwd: '../../../../',
    reuseExistingServer: true,
    timeout: 60000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
