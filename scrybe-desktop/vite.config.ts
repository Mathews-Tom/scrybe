/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri reads `build.devUrl` from `src-tauri/tauri.conf.json`; the port
// here is the other half of that contract. `strictPort` makes a port
// collision fail loudly instead of silently serving the shell somewhere
// the host will not look.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2023", emptyOutDir: true },
  test: {
    globals: true,
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    restoreMocks: true,
  },
});
