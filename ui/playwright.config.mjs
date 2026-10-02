import { defineConfig } from "@playwright/test";

// ブラウザ単体（モックバックエンド）の UI を、実際のブラウザで操作して確かめる。
// ローカルで Playwright 管理外の Chromium を使うときは PW_CHROMIUM に実行ファイルのパスを入れる。
export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.e2e.mjs",
  timeout: 60_000,
  expect: { timeout: 10_000 },
  fullyParallel: true,
  workers: process.env.CI ? 2 : undefined,
  retries: 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: "http://localhost:1420",
    viewport: { width: 1500, height: 850 },
    trace: "retain-on-failure",
    launchOptions: {
      executablePath: process.env.PW_CHROMIUM || undefined,
      args: ["--no-sandbox"],
    },
  },
  webServer: {
    command: "pnpm dev",
    url: "http://localhost:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
