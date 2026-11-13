import { defineConfig } from "vite";

// Vite options tailored for Tauri development and testing.
export default defineConfig({
  // 1. Prevent Vite from obscuring Rust errors.
  clearScreen: false,
  server: {
    // 2. Tauri expects a fixed port; fail if that port is unavailable.
    port: 1420,
    strictPort: true,
    watch: {
      // 3. Tell Vite to ignore watching `src-tauri`.
      ignored: ["**/src-tauri/**"],
    },
  },
});