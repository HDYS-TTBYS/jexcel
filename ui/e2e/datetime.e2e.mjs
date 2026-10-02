import { test, expect } from "@playwright/test";

test("日時列のタイムゾーンを一括変換する", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const cell = (i) => p.locator(`.ag-row[row-index="${i}"] [col-id]:not([col-id="__delete"])`).first();
  const cells = async () => (await p.locator(".ag-row [col-id]:not([col-id='__delete'])").allInnerTexts()).map((t) => t.trim());
  const type = async (i, v) => {
    await cell(i).dblclick();
    await p.keyboard.press("Control+a");
    await p.keyboard.type(v);
    await p.keyboard.press("Enter");
  };

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");

  // 列 1 を日時型にする
  await p.getByRole("button", { name: "列の定義" }).click();
  const dlg = p.getByRole("dialog");
  check("日時型でない列には変換ボタンがない", (await dlg.getByRole("button", { name: /タイムゾーンを変換/ }).count()) === 0);
  await dlg.getByLabel("型").first().selectOption("dateTime");
  await dlg.getByRole("button", { name: "適用" }).first().click();
  await expect(dlg.getByRole("button", { name: /タイムゾーンを変換/ })).toHaveCount(1);
  await dlg.getByRole("button", { name: "閉じる" }).click();

  // UTC の値を 2 行入れる（1 行目は日付をまたぐ）
  await type(0, "2026-10-02T20:00:00Z");
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  await type(1, "2026-10-02T01:30:00.250Z");
  await p.getByRole("button", { name: "+ 行を追加" }).click(); // 空の行
  check("入力した値が入っている", (await cells()).slice(0, 2).join("|") === "2026-10-02T20:00:00Z|2026-10-02T01:30:00.250Z", JSON.stringify(await cells()));

  // 不正なオフセットは拒否される
  await p.getByRole("button", { name: "列の定義" }).click();
  await p.getByRole("button", { name: /タイムゾーンを変換/ }).click();
  const prompt = p.getByRole("dialog").last();
  await prompt.locator("input").fill("JST");
  await prompt.getByRole("button", { name: "OK" }).click();
  await expect(p.locator(".toast.error")).toContainText("オフセットは");
  check("拒否されても値は変わらない", (await cells())[0] === "2026-10-02T20:00:00Z");

  // +09:00 に変換
  await p.getByRole("button", { name: /タイムゾーンを変換/ }).click();
  await p.getByRole("dialog").last().locator("input").fill("+09:00");
  await p.getByRole("dialog").last().getByRole("button", { name: "OK" }).click();
  await expect(p.getByRole("dialog").last()).toContainText("2 件を変換しました");
  await p.getByRole("dialog").last().getByRole("button", { name: "OK" }).click();
  await p.getByRole("button", { name: "閉じる" }).click();
  check("日付をまたいで +09:00 になる", (await cells())[0] === "2026-10-03T05:00:00+09:00", JSON.stringify(await cells()));
  check("小数秒は残る", (await cells())[1] === "2026-10-02T10:30:00.250+09:00", JSON.stringify(await cells()));
  check("未保存になる", ((await p.getByRole("button", { name: /^保存/ }).first().innerText()) || "").includes("*"));

  // もう一度同じ変換: 何も変わらない
  await p.getByRole("button", { name: "列の定義" }).click();
  await p.getByRole("button", { name: /タイムゾーンを変換/ }).click();
  await p.getByRole("dialog").last().getByRole("button", { name: "OK" }).click();
  await expect(p.getByRole("dialog").last()).toContainText("0 件を変換しました");
  check("すでに同じオフセットの件数が出る", (await p.getByRole("dialog").last().innerText()).includes("2 件はすでに"));
  await p.getByRole("dialog").last().getByRole("button", { name: "OK" }).click();

  // Z に戻せる
  await p.getByRole("button", { name: /タイムゾーンを変換/ }).click();
  await p.getByRole("dialog").last().locator("input").fill("Z");
  await p.getByRole("dialog").last().getByRole("button", { name: "OK" }).click();
  await p.getByRole("dialog").last().getByRole("button", { name: "OK" }).click();
  await p.getByRole("button", { name: "閉じる" }).click();
  check("Z に戻せる", (await cells()).slice(0, 2).join("|") === "2026-10-02T20:00:00Z|2026-10-02T01:30:00.250Z", JSON.stringify(await cells()));

  expect(errs).toEqual([]);
});
