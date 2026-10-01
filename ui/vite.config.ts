import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri の dev サーバは固定ポートを前提にする
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // モックが crates/jxcel-macro の prelude.js を ?raw で読むので、一つ上の階層まで許可する
  server: { port: 1420, strictPort: true, fs: { allow: [".."] } },
  test: { environment: "node" },
});
