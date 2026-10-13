import { defineConfig } from "vite";

// Tauri expects a fixed port in dev and plain static files in dist/ for builds.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: {
    outDir: "dist",
    target: ["es2022", "chrome110", "safari15"],
    sourcemap: false,
    reportCompressedSize: false,
  },
});
