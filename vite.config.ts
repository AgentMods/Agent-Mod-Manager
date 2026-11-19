import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
  ],
  resolve: {alias: {"@": fileURLToPath(new URL("./src", import.meta.url))}},
  // Parallel test runs set VITE_CACHE_DIR so their dependency optimizers do not collide.
  cacheDir: process.env.VITE_CACHE_DIR || "node_modules/.vite",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // Playwright writes traces/screenshots under output/ while tests run; watching them reloads the page mid-test.
    watch: { ignored: ["**/src-tauri/**", "**/output/**", "**/tmp/**", "**/test-results/**", "**/playwright-report/**"] },
  },
});
