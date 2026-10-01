import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri の dev サーバは固定ポートを前提にする
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  test: { environment: "node" },
});
