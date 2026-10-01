import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterAll, describe, expect, it } from "vitest";

// 同梱のサンプルマクロが、エディタ（Monaco）と同じ TypeScript の厳格な型チェックを通ること。
// Rust 側のテストはサンプルを「実行」して確かめる。ここは「エディタで赤線が出ない」ことを確かめる。
const samplesDir = resolve(__dirname, "../../crates/jxcel-macro/samples");
const api = resolve(__dirname, "jxcelApi.d.ts.txt");
const dir = mkdtempSync(join(tmpdir(), "jxcel-typecheck-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

function setup(extra?: Record<string, string>) {
  for (const f of readdirSync(dir)) rmSync(join(dir, f), { force: true });
  copyFileSync(api, join(dir, "jxcelApi.d.ts"));
  for (const f of readdirSync(samplesDir)) copyFileSync(join(samplesDir, f), join(dir, f));
  for (const [name, src] of Object.entries(extra ?? {})) writeFileSync(join(dir, name), src);
  writeFileSync(
    join(dir, "tsconfig.json"),
    JSON.stringify({ compilerOptions: { target: "ES2020", module: "ESNext", strict: true, noEmit: true, lib: ["ES2020"], types: [], moduleResolution: "bundler" }, include: ["*.ts"] }),
  );
}

function tsc(): { ok: boolean; output: string } {
  try {
    const out = execFileSync(resolve(__dirname, "../node_modules/.bin/tsc"), ["-p", join(dir, "tsconfig.json")], { encoding: "utf8", stdio: "pipe" });
    return { ok: true, output: out };
  } catch (e) {
    const err = e as { stdout?: string; stderr?: string };
    return { ok: false, output: `${err.stdout ?? ""}${err.stderr ?? ""}` };
  }
}

describe("サンプルマクロの型チェック", () => {
  it("全サンプルが型エラーなし", () => {
    setup();
    const r = tsc();
    expect(r.output).toBe("");
    expect(r.ok).toBe(true);
  });

  it("検査が機能している: 存在しない関数・型違いはエラーになる", () => {
    setup({
      "bad.ts": 'export default (jx: Jxcel) => { std.nope(); jx.sheet(1); const n: number = std.text.trim("a"); return n; }\n',
    });
    const r = tsc();
    expect(r.ok).toBe(false);
    expect(r.output).toContain("nope");
    expect(r.output).toMatch(/number/);
  });
});
