import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;
const playwrightStableServer = process.env.AMM_PLAYWRIGHT_STABLE_SERVER === "1";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
  ],
  resolve: {alias: {"@": fileURLToPath(new URL("./src", import.meta.url))}},
  // Parallel test runs set VITE_CACHE_DIR so their dependency optimizers do not collide.
  cacheDir: process.env.VITE_CACHE_DIR || "node_modules/.vite",
  // The UI suite needs a stable document: source edits and late-discovered
  // dependencies must not trigger Vite's HMR/full-reload websocket mid-test.
  // Scan every application module up front so lazy routes cannot restart the
  // optimizer after the first browser navigation.
  ...(playwrightStableServer ? { optimizeDeps: { entries: ["src/**/*.tsx", "src/**/*.ts"] } } : {}),
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: playwrightStableServer ? false : host ? { protocol: "ws", host, port: 1421 } : undefined,
    // Playwright writes traces/screenshots under output/ while tests run; watching them reloads the page mid-test.
    watch: playwrightStableServer ? null : { ignored: ["**/src-tauri/**", "**/output/**", "**/tmp/**", "**/test-results/**", "**/playwright-report/**"] },
  },
});
