import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1200, height: 700 } });

test("未保存の警告（終了・新規・開く）", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const closed = () => p.evaluate(() => !!window.__closed);
  const dlgTitle = () => p.getByRole("dialog").locator("h2").innerText();

  await p.goto("/");
  await p.waitForSelector("text=新規作成");

  // 1. ファイルなし → 確認なしで閉じる
  await p.evaluate(() => window.__requestClose());
  await p.waitForTimeout(100);
  check("ファイルなしなら確認なしで閉じる", await closed());
  await p.evaluate(() => (window.__closed = false));

  // 2. 新規作成（未保存）→ 閉じる要求で警告、キャンセルで閉じない
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");
  await p.getByRole("button", { name: "+ 行を追加" }).click(); // 新規直後は変更なしなので、編集して未保存にする
  await p.evaluate(() => window.__requestClose());
  check("未保存で閉じると警告", (await dlgTitle()) === "終了前の確認");
  await p.getByRole("button", { name: "キャンセル" }).click();
  check("キャンセルすると閉じない", !(await closed()));

  // 3. 破棄して続行 → 閉じる
  await p.evaluate(() => window.__requestClose());
  await p.getByRole("button", { name: "破棄して続行" }).click();
  await p.waitForTimeout(100);
  check("破棄して続行で閉じる", await closed());
  await p.evaluate(() => (window.__closed = false));

  // 4. 保存して続行（保存先ありなので保存→閉じる）。モックの保存先ピッカーは常に成功する
  await p.evaluate(() => window.__requestClose());
  await p.getByRole("button", { name: "保存して続行" }).click();
  await p.waitForTimeout(300);
  check("保存して続行で保存後に閉じる", await closed());
  check("保存済み表示になる", !(await p.getByRole("button", { name: /^保存/ }).first().innerText()).includes("*"));
  await p.evaluate(() => (window.__closed = false));

  // 5. 保存済みなら閉じるのに警告なし
  await p.evaluate(() => window.__requestClose());
  await p.waitForTimeout(100);
  check("保存済みなら確認なしで閉じる", await closed());
  await p.evaluate(() => (window.__closed = false));

  // 6. 変更 → 「新規」で警告。破棄して続行で新規ダイアログへ
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  await p.getByRole("button", { name: "新規", exact: true }).click();
  check("新規で警告", (await dlgTitle()) === "新規作成前の確認");
  await p.getByRole("button", { name: "破棄して続行" }).click();
  check("破棄後に新規ファイル名ダイアログ", (await dlgTitle()) === "新しいファイル");
  await p.getByRole("button", { name: "キャンセル" }).click();

  // 7. 変更 → 「開く」で警告、キャンセルで元の状態のまま
  await p.getByRole("button", { name: "開く" }).click();
  check("開くで警告", (await dlgTitle()) === "ファイルを開く前の確認");
  await p.getByRole("button", { name: "キャンセル" }).click();
  const rows = await p.locator(".ag-row").count(); check("キャンセル後もグリッドが残る（新規1行＋編集で追加した1行＋最後の追加1行）", rows === 3);


  expect(errs).toEqual([]);
});
