import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1200, height: 700 } });

test("新規ファイルの未保存扱いと保存", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const closed = () => p.evaluate(() => !!window.__closed);
  const reset = () => p.evaluate(() => (window.__closed = false));
  const saveLabel = async () => (await p.getByRole("button", { name: /^保存/ }).first().innerText()).trim();

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");

  // 1. 新規直後（未編集）: 変更ありの印がなく、閉じるのに警告が出ない
  check("新規直後は「保存」に * が付かない", (await saveLabel()) === "保存");
  await p.evaluate(() => window.__requestClose());
  await p.waitForTimeout(100);
  check("未編集の新規ファイルは確認なしで閉じる", (await closed()) && (await p.getByRole("dialog").count()) === 0);
  await reset();

  // 2. 未編集でも「新規」「開く」で警告が出ない（新規ダイアログが直接出る）
  await p.getByRole("button", { name: "新規", exact: true }).click();
  check("未編集なら「新規」で警告なし", (await p.getByRole("dialog").locator("h2").innerText()) === "新しいファイル");
  await p.getByRole("button", { name: "キャンセル" }).click();

  // 3. 編集すると変更あり: 印が付き、閉じる/新規で警告
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  check("編集すると * が付く", (await saveLabel()) === "保存 *");
  await p.evaluate(() => window.__requestClose());
  check("編集後は閉じると警告", (await p.getByRole("dialog").locator("h2").innerText()) === "終了前の確認");
  await p.getByRole("button", { name: "キャンセル" }).click();
  check("キャンセルで閉じない", !(await closed()));

  // 4. 未編集の新規ファイルでも保存はできる
  await p.getByRole("button", { name: "新規", exact: true }).click();
  await p.getByRole("button", { name: "破棄して続行" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");
  check("作り直した新規は変更なし", (await saveLabel()) === "保存");
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(300);
  check("未編集の新規も保存でき、保存先が表示される", (await p.locator(".title").innerText()).includes("/mock/"));
  await p.getByRole("button", { name: "履歴", exact: true }).click();
  await p.waitForSelector(".commits li");
  check("保存すると履歴が 1 件できる", (await p.locator(".commits li").count()) === 1);


  expect(errs).toEqual([]);
});
