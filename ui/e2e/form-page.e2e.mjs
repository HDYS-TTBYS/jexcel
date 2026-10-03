// LAN フォーム配信の、回答者側の画面（ブラウザで開くフォーム）。本物の Rust のサーバー
// （crates/jxcel-app/examples/serve_forms.rs）を起動して、実際に HTTP で送信する。
// 事前に `cargo build -p jxcel-app --example serve_forms` が必要。実行ファイルの場所は SERVE_FORMS_BIN で変えられる。
import { test, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const exe = process.platform === "win32" ? "serve_forms.exe" : "serve_forms";
const bin = process.env.SERVE_FORMS_BIN || path.resolve(here, "../../target/debug/examples", exe);

test.use({ viewport: { width: 390, height: 844 } });

// 回答者の端末のタイムゾーンで入力した時刻が、そのオフセット付きで残る（10 月 2 日: 東京は +09:00、ニューヨークは夏時間で -04:00）
for (const [timezoneId, offset] of [["Asia/Tokyo", "+09:00"], ["America/New_York", "-04:00"]]) {
  test.describe(timezoneId, () => {
    test.use({ timezoneId });
    test(`フォーム配信: 回答者の画面から送信できる（${timezoneId}）`, async ({ page }) => runFormPage(page, offset));
  });
}

async function runFormPage(p, offset) {
  // CI では必ず実行する（実行ファイルが無ければ、起動に失敗してテストが落ちる）
  test.skip(!existsSync(bin) && !process.env.CI, `${bin} がありません（cargo build -p jxcel-app --example serve_forms）`);

  const srv = spawn(bin, [], { stdio: ["pipe", "pipe", "inherit"] });
  let out = "";
  srv.stdout.on("data", (d) => (out += d));
  const waitFor = async (pred, what) => {
    const t = Date.now();
    while (!pred()) {
      if (Date.now() - t > 10_000) throw new Error(`待ち時間切れ: ${what}\n${out}`);
      await new Promise((r) => setTimeout(r, 50));
    }
  };
  try {
    await waitFor(() => out.includes("READY"), "サーバーの起動");
    const url = out.match(/URL (\S+)/)[1];
    const errs = [];
    p.on("pageerror", (e) => errs.push(e.message));
    const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);

    await p.goto(url);
    await p.waitForSelector("form button");
    check("題名が出る", (await p.locator("h1").innerText()) === "来客受付");
    check("title も題名", (await p.title()) === "来客受付");
    check("7 つの入力欄", (await p.locator("form label").count()) === 7);
    check("必須に * が付く", (await p.locator("form label").first().innerText()).includes("*"));
    const types = await p.locator("form input, form select").evaluateAll((l) => l.map((e) => e.type));
    check("入力の種類", JSON.stringify(types) === JSON.stringify(["text", "number", "text", "select-one", "date", "datetime-local", "checkbox"]), JSON.stringify(types));

    // 必須を空のまま送信 → サーバーのメッセージが出る
    await p.locator("form button[type=submit]").click();
    await expect(p.locator("#msg")).toContainText("氏名 は必須です");
    check("エラーは赤", ((await p.locator("#msg").getAttribute("class")) || "").includes("err"));

    // 型に合わない値
    await p.locator("label", { hasText: "氏名" }).locator("input").fill("山田 太郎");
    await p.locator("label", { hasText: "予算" }).locator("input").fill("abc");
    await p.locator("form button[type=submit]").click();
    await expect(p.locator("#msg")).toContainText("予算");
    check("拒否された回答は表に入らない", !out.includes("ROWS"));

    // 正しい入力
    await p.locator("label", { hasText: "人数" }).locator("input").fill("4");
    await p.locator("label", { hasText: "予算" }).locator("input").fill("12000.50");
    await p.locator("label", { hasText: "区分" }).locator("select").selectOption("法人");
    await p.locator("label", { hasText: "来訪日" }).locator("input").fill("2026-10-02");
    await p.locator("label", { hasText: "来訪時刻" }).locator("input").fill("2026-10-02T10:30");
    await p.locator("label", { hasText: "了承済み" }).locator("input").check();
    await p.locator("form button[type=submit]").click();
    await expect(p.locator("#msg")).toContainText("送信しました");
    await waitFor(() => out.includes("ROWS"), "回答の到着");
    const rows = JSON.parse(out.match(/ROWS (.*)/)[1]);
    check("1 行追加された", rows.length === 1);
    const r = rows[0];
    check("値が列の型で入る", r.name === "山田 太郎" && r.people === 4 && r.price === "12000.50" && r.kind === "法人" && r.day === "2026-10-02" && r.memo === true, JSON.stringify(r));
    check("日時は入力した時刻のまま、端末のオフセット付きで残る", r.at === `2026-10-02T10:30:00${offset}`, r.at);

    // 送信後は内容が残り、そのまま直せる（修正）
    const field = (name) => p.locator("label", { hasText: name }).locator("input, select");
    check("送信後も入力は残り、修正の状態になる", (await field("氏名").inputValue()) === "山田 太郎" && (await p.locator("form button[type=submit]").innerText()) === "修正を送信");
    await field("人数").fill("5");
    await p.locator("form button[type=submit]").click();
    await expect(p.locator("#msg")).toContainText("修正を送信しました");
    await waitFor(() => (out.match(/ROWS /g) || []).length >= 2, "修正の反映");
    const edited = JSON.parse([...out.matchAll(/ROWS (.*)/g)].pop()[1]);
    check("修正は行を増やさず、値だけ変える", edited.length === 1 && edited[0].people === 5 && edited[0].name === "山田 太郎", JSON.stringify(edited));
    // 開き直しても、同じ回答を直せる（修正用トークンをこの端末に覚えている）
    await p.reload();
    await expect(p.locator("#msg")).toContainText("送信済みの回答です");
    check("開き直すと前回の内容が入る", (await field("人数").inputValue()) === "5" && (await field("来訪時刻").inputValue()) === "2026-10-02T10:30" && (await field("了承済み").isChecked()));
    check("修正のボタン", (await p.locator("form button[type=submit]").innerText()) === "修正を送信" && (await p.getByRole("button", { name: "新しい回答として送る" }).isVisible()));
    // 新しい回答として送る → 空になり、送信は新しい行になる
    await p.getByRole("button", { name: "新しい回答として送る" }).click();
    check("新しい回答は空から始まる", (await field("氏名").inputValue()) === "" && (await p.locator("form button[type=submit]").innerText()) === "送信");

    // トークンが違えば、フォームの存在も分からない
    const bad = await p.goto(url.replace(/[0-9a-f]{32}$/, "0".repeat(32)));
    check("トークン違いは 404", bad.status() === 404);
    check("エラー画面に回答の内容がない", !(await p.content()).includes("山田"));
    expect(errs).toEqual([]);
  } finally {
    srv.stdin.write("\n");
    setTimeout(() => srv.kill(), 1000).unref();
  }
}

// 合言葉つきの配信: 合言葉を入れるまでフォームの中身は出ず、間違いは数えられ、続くとロックされる
test("フォーム配信: 合言葉が要る・間違いが続くとロックされる", async ({ page: p }) => {
  test.skip(!existsSync(bin) && !process.env.CI, `${bin} がありません（cargo build -p jxcel-app --example serve_forms）`);
  const srv = spawn(bin, ["0", "himitsu-1234"], { stdio: ["pipe", "pipe", "inherit"] });
  let out = "";
  srv.stdout.on("data", (d) => (out += d));
  const waitFor = async (pred, what) => {
    const t = Date.now();
    while (!pred()) {
      if (Date.now() - t > 10_000) throw new Error(`待ち時間切れ: ${what}\n${out}`);
      await new Promise((r) => setTimeout(r, 50));
    }
  };
  try {
    await waitFor(() => out.includes("READY"), "サーバーの起動");
    const url = out.match(/URL (\S+)/)[1];
    const errs = [];
    p.on("pageerror", (e) => errs.push(e.message));

    await p.goto(url);
    await expect(p.locator("h1")).toHaveText("合言葉が必要です");
    expect(await p.content()).not.toContain("来客受付");
    expect(await p.locator("form label").count()).toBe(1);

    // 間違い → メッセージが出て、入力欄はそのまま
    await p.getByLabel("合言葉").fill("wrong-code");
    await p.getByRole("button", { name: "開く" }).click();
    await expect(p.locator("#msg")).toContainText("合言葉が違います");
    await expect(p.locator("h1")).toHaveText("合言葉が必要です");

    // 正しい合言葉 → フォームが開く。回答できる
    await p.getByLabel("合言葉").fill("himitsu-1234");
    await p.getByRole("button", { name: "開く" }).click();
    await expect(p.locator("h1")).toHaveText("来客受付");
    await p.locator("label", { hasText: "氏名" }).locator("input").fill("鈴木");
    await p.locator("label", { hasText: "人数" }).locator("input").fill("2");
    await p.locator("label", { hasText: "区分" }).locator("select").selectOption("個人");
    await p.locator("form button[type=submit]").click();
    await expect(p.locator("#msg")).toContainText("送信しました");
    await waitFor(() => out.includes("ROWS"), "回答の到着");

    // 同じタブで開き直しても、合言葉を覚えている（sessionStorage）
    await p.reload();
    await expect(p.locator("h1")).toHaveText("来客受付");

    // 合言葉なしの直接の問い合わせは 401（数えない）。画面での間違い 1 回と合わせて 8 回間違えるとロック（429）され、正しい合言葉でも通らない
    const def = url + "/def";
    expect((await p.request.get(def)).status()).toBe(401);
    for (let i = 0; i < 7; i++) {
      expect((await p.request.get(def, { headers: { "x-jxcel-code": "nope-" + i } })).status()).toBe(401);
    }
    const locked = await p.request.get(def, { headers: { "x-jxcel-code": "himitsu-1234" } });
    expect(locked.status()).toBe(429);
    expect(Number(locked.headers()["retry-after"])).toBeGreaterThan(0);
    expect(errs).toEqual([]);
  } finally {
    srv.stdin.write("\n");
    setTimeout(() => srv.kill(), 1000).unref();
  }
});

// 修正の設定: 受け付けないフォームは修正の状態にならず、期限つきなら案内に期間が出る
for (const [label, args, expectHint] of [
  ["修正を受け付けない", ["0", "", "no-edit"], null],
  ["期限つき（30 分）", ["0", "", "30"], "送信から 30 分以内"],
  ["期限つき（2 時間）", ["0", "", "120"], "送信から 2 時間以内"],
]) {
  test(`フォーム配信: ${label}`, async ({ page: p }) => {
    test.skip(!existsSync(bin) && !process.env.CI, `${bin} がありません（cargo build -p jxcel-app --example serve_forms）`);
    const srv = spawn(bin, args, { stdio: ["pipe", "pipe", "inherit"] });
    let out = "";
    srv.stdout.on("data", (d) => (out += d));
    const waitFor = async (pred, what) => {
      const t = Date.now();
      while (!pred()) {
        if (Date.now() - t > 10_000) throw new Error(`待ち時間切れ: ${what}\n${out}`);
        await new Promise((r) => setTimeout(r, 50));
      }
    };
    try {
      await waitFor(() => out.includes("READY"), "サーバーの起動");
      const url = out.match(/URL (\S+)/)[1];
      const errs = [];
      p.on("pageerror", (e) => errs.push(e.message));
      await p.goto(url);
      await p.waitForSelector("form button");
      await p.locator("label", { hasText: "氏名" }).locator("input").fill("山田");
      await p.locator("label", { hasText: "人数" }).locator("input").fill("2");
      await p.locator("label", { hasText: "区分" }).locator("select").selectOption("個人");
      await p.locator("form button[type=submit]").click();
      await expect(p.locator("#msg")).toContainText("送信しました");
      await waitFor(() => out.includes("ROWS"), "回答の到着");
      const msg = await p.locator("#msg").innerText();
      if (expectHint) {
        expect(msg).toContain(expectHint);
        expect(await p.locator("form button[type=submit]").innerText()).toBe("修正を送信");
      } else {
        // 修正用の鍵が渡らないので、修正の状態にならず、内容は空に戻る。直すための案内も出ない
        expect(msg).not.toContain("修正");
        expect(await p.locator("form button[type=submit]").innerText()).toBe("送信");
        expect(await p.getByRole("button", { name: "新しい回答として送る" }).isVisible()).toBe(false);
        expect(await p.locator("label", { hasText: "氏名" }).locator("input").inputValue()).toBe("");
        await p.reload();
        await p.waitForSelector("form button");
        expect(await p.locator("label", { hasText: "氏名" }).locator("input").inputValue()).toBe("");
      }
      expect(errs).toEqual([]);
    } finally {
      srv.stdin.write("\n");
      setTimeout(() => srv.kill(), 1000).unref();
    }
  });
}

// 回答者ごとの合言葉: 1 人 1 件で、別の端末（別のブラウザ）でも同じ合言葉で自分の回答を直せる
test("フォーム配信: 回答者ごとの合言葉", async ({ browser }) => {
  test.skip(!existsSync(bin) && !process.env.CI, `${bin} がありません（cargo build -p jxcel-app --example serve_forms）`);
  const srv = spawn(bin, ["0", "", "", "yamada-1,sato-22"], { stdio: ["pipe", "pipe", "inherit"] });
  let out = "";
  srv.stdout.on("data", (d) => (out += d));
  const waitFor = async (pred, what) => {
    const t = Date.now();
    while (!pred()) {
      if (Date.now() - t > 10_000) throw new Error(`待ち時間切れ: ${what}\n${out}`);
      await new Promise((r) => setTimeout(r, 50));
    }
  };
  const open = async (url, code) => {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 } });
    const p = await ctx.newPage();
    await p.goto(url);
    await expect(p.locator("h1")).toHaveText("合言葉が必要です");
    await p.getByLabel("合言葉").fill(code);
    await p.getByRole("button", { name: "開く" }).click();
    await expect(p.locator("h1")).toHaveText("来客受付");
    return { ctx, p, field: (n) => p.locator("label", { hasText: n }).locator("input, select") };
  };
  const rowsNow = () => JSON.parse([...out.matchAll(/ROWS (.*)/g)].pop()[1]);
  try {
    await waitFor(() => out.includes("READY"), "サーバーの起動");
    const url = out.match(/URL (\S+)/)[1];

    // 山田さんが送信する。1 人 1 件なので、新しい回答として送り直すボタンは出ない
    const a = await open(url, "yamada-1");
    await a.field("氏名").fill("山田");
    await a.field("人数").fill("2");
    await a.field("区分").selectOption("個人");
    await a.p.locator("form button[type=submit]").click();
    await expect(a.p.locator("#msg")).toContainText("送信しました");
    await waitFor(() => out.includes("ROWS"), "回答の到着");
    expect(await a.p.locator("form button[type=submit]").innerText()).toBe("修正を送信");
    expect(await a.p.getByRole("button", { name: "新しい回答として送る" }).isVisible()).toBe(false);

    // 別の端末（別のブラウザ）で同じ合言葉 → 前回の回答が入っていて、直せる
    const b = await open(url, "yamada-1");
    await expect(b.p.locator("#msg")).toContainText("送信済みの回答です");
    expect(await b.field("氏名").inputValue()).toBe("山田");
    expect(await b.field("人数").inputValue()).toBe("2");
    await b.field("人数").fill("5");
    await b.p.locator("form button[type=submit]").click();
    await expect(b.p.locator("#msg")).toContainText("修正を送信しました");
    await waitFor(() => rowsNow()[0].people === 5, "別の端末からの修正");
    expect(rowsNow().length).toBe(1);

    // 佐藤さんは別の回答。山田さんの回答は見えない（空の新しいフォーム）
    const c = await open(url, "sato-22");
    expect(await c.field("氏名").inputValue()).toBe("");
    expect(await c.p.locator("form button[type=submit]").innerText()).toBe("送信");
    await c.field("氏名").fill("佐藤");
    await c.field("区分").selectOption("法人");
    await c.p.locator("form button[type=submit]").click();
    await expect(c.p.locator("#msg")).toContainText("送信しました");
    await waitFor(() => rowsNow().length === 2, "佐藤さんの回答");
    expect(rowsNow().map((r) => r.name)).toEqual(["山田", "佐藤"]);

    // 合言葉が違えば入れない
    const d = await browser.newContext();
    const dp = await d.newPage();
    await dp.goto(url);
    await dp.getByLabel("合言葉").fill("nobody-99");
    await dp.getByRole("button", { name: "開く" }).click();
    await expect(dp.locator("#msg")).toContainText("合言葉が違います");
    for (const x of [a, b, c]) await x.ctx.close();
    await d.close();
  } finally {
    srv.stdin.write("\n");
    setTimeout(() => srv.kill(), 1000).unref();
  }
});
