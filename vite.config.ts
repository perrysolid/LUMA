/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import { resolve } from "node:path";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  clearScreen: false,
  build: {
    rollupOptions: {
      // panel + one overlay page (instantiated once per display)
      input: { main: resolve(__dirname, "index.html"), overlay: resolve(__dirname, "overlay.html") },
    },
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
}));
