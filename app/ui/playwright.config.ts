import { defineConfig } from "@playwright/test";

// Scenarios run against the Vite dev server with the fake backend (`src/fake.ts`).
export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  reporter: [["list"]],
  use: {
    baseURL: "http://127.0.0.1:1420",
    viewport: { width: 880, height: 640 },
    colorScheme: "light",
    reducedMotion: "reduce",
    timezoneId: "UTC",
    locale: "en-US",
  },
  webServer: {
    command: "pnpm dev",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: true,
  },
});
