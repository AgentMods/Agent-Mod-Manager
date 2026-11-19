import { defineConfig } from "@playwright/test";

// Parallel local runs can set PW_PORT so each run owns its dev server and artifact folders.
const port = Number(process.env.PW_PORT ?? 1430);
const suffix = port === 1430 ? "" : `-${port}`;

export default defineConfig({
  testDir: "./tests/ui",
  timeout: 30_000,
  expect: { timeout: 5000 },
  fullyParallel: true,
  reporter: [["list"], ["html", { outputFolder: `output/playwright/report${suffix}`, open: "never" }]],
  outputDir: `output/playwright/results${suffix}`,
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "desktop-1280", use: { browserName: "chromium", viewport: { width: 1280, height: 800 } } },
    { name: "desktop-1440", use: { browserName: "chromium", viewport: { width: 1440, height: 900 } } },
    { name: "narrow-390", use: { browserName: "chromium", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true } },
  ],
  webServer: {
    command: `yarn dev --host 127.0.0.1 --port ${port}`,
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: false,
    env: { VITE_CACHE_DIR: `node_modules/.vite-pw-${port}` },
    timeout: 30_000,
  },
});
