import { defineConfig, devices } from "@playwright/test";

// E2E for the `sema web` dev server. Unlike the main config, there is NO
// webServer here — each spec spawns the real `sema` binary itself (so hot-reload
// specs can own and mutate the served files). Kept in its own directory so the
// main suite (./e2e/tests) doesn't pick it up.
export default defineConfig({
  testDir: "./e2e/dev-server",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  timeout: 120_000,
  use: {
    trace: "on-first-retry",
  },
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
