import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri expects a fixed port in dev and plain static files in dist/ for builds.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: {
    outDir: "dist",
    target: ["es2022", "chrome110", "safari15"],
    sourcemap: false,
    reportCompressedSize: false,
  },
  test: { include: ["src/**/*.test.{ts,tsx}"], environment: "node" },
});
