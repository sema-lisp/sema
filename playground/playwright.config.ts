import { defineConfig } from '@playwright/test';

// Port is overridable (PW_PORT) so a run can dodge a port already taken by
// another local dev server; defaults to the conventional 8787.
const port = Number(process.env.PW_PORT ?? 8787);

export default defineConfig({
  testDir: './tests',
  timeout: 60000,
  use: {
    baseURL: `http://localhost:${port}`,
  },
  webServer: {
    command: `npx serve -l ${port}`,
    port,
    reuseExistingServer: true,
  },
});
