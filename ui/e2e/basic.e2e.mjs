import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1200, height: 700 } });

test("基本操作（列の定義・入力・保存・履歴）", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.getByRole("button", { name: "列の定義" }).click();
  const dlg = p.getByRole("dialog");
  await dlg.getByLabel("列名").fill("数量");
  await dlg.getByLabel("型").first().selectOption("int");
  await dlg.getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "+ 列を追加" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();
  // セル入力: 不正値 → エラー、正しい値 → 反映
  const cell = p.locator('.ag-row[row-index="0"] [col-id]:not([col-id="__delete"])').first();
  await cell.dblclick();
  await p.keyboard.type("abc");
  await p.keyboard.press("Enter");
  await p.waitForSelector(".toast.error");
  check("型に合わない値は拒否される", (await p.locator(".toast.error").innerText()).includes("整数"));
  check("拒否された値はセルに入らない", (await cell.innerText()).trim() === "");
  await cell.dblclick();
  await p.keyboard.press("Control+a");
  await p.keyboard.type("5");
  await p.keyboard.press("Enter");
  await p.waitForTimeout(300);
  check("正しい値はセルに入る", (await cell.innerText()).trim() === "5");
  // 保存 → 変更 → 保存 → 履歴
  await p.getByPlaceholder("変更メモ").fill("初回");
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(300);
  await cell.dblclick(); await p.keyboard.press("Control+a"); await p.keyboard.type("8"); await p.keyboard.press("Enter");
  await p.getByPlaceholder("変更メモ").fill("数量を更新");
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.getByRole("button", { name: "履歴", exact: true }).click();
  await p.waitForSelector(".commits li");
  await expect(p.locator(".commits li")).toHaveCount(2);
  const msgs = await p.locator(".commits .msg").allInnerTexts();
  check("履歴に変更メモが残る", msgs.includes("初回") && msgs.includes("数量を更新"), JSON.stringify(msgs));

  expect(errs).toEqual([]);
});
