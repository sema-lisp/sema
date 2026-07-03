import { defineConfig, devices } from "@playwright/test";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

export default defineConfig({
  testDir: ".",
  fullyParallel: false,
  forbidOnly: false,
  retries: 0,
  workers: 1,
  outputDir: "../../test-results/browser-bench",
  use: {
    baseURL: "http://localhost:5173",
    trace: "off",
  },
  webServer: [
    {
      command: "vite e2e/fixtures --port 5173",
      cwd: packageDir,
      url: "http://localhost:5173",
      reuseExistingServer: true,
      timeout: 120_000,
    },
    {
      command: "npx tsx e2e/mock-proxy.ts --port=3002",
      cwd: packageDir,
      url: "http://localhost:3002/health",
      reuseExistingServer: true,
      timeout: 120_000,
    },
  ],
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
          ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
          : {},
      },
    },
  ],
});
