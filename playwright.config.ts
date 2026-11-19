import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/ui",
  timeout: 30_000,
  expect: { timeout: 5000 },
  fullyParallel: true,
  reporter: [["list"], ["html", { outputFolder: "output/playwright/report", open: "never" }]],
  outputDir: "output/playwright/results",
  use: {
    baseURL: "http://127.0.0.1:1430",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "desktop-1280", use: { browserName: "chromium", viewport: { width: 1280, height: 800 } } },
    { name: "desktop-1440", use: { browserName: "chromium", viewport: { width: 1440, height: 900 } } },
    { name: "narrow-390", use: { browserName: "chromium", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
  ],
  webServer: {
    command: "yarn dev --host 127.0.0.1 --port 1430",
    url: "http://127.0.0.1:1430",
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
