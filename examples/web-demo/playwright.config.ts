import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  use: {
    baseURL: "http://localhost:5180",
    trace: "on-first-retry",
  },
  webServer: [
    {
      command: "npx vite --port 5180",
      url: "http://localhost:5180",
      reuseExistingServer: !process.env.CI,
      timeout: 15_000,
    },
    {
      command: "npx tsx proxy.ts",
      url: "http://localhost:3002/health",
      reuseExistingServer: !process.env.CI,
      timeout: 15_000,
    },
  ],
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],
});
