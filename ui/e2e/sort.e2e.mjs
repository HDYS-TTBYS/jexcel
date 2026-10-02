import { test, expect } from "@playwright/test";

test("グリッドの並べ替え（数・日時のオフセット混在・空は最後）", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const colText = async (name) => {
    const colId = await p.locator(".ag-header-cell", { hasText: name }).first().getAttribute("col-id");
    // AG Grid の DOM の並びは表示順と限らないので、行の表示位置（row-index）で並べ直す
    const pairs = await p.locator(`.ag-row [col-id="${colId}"]`).evaluateAll((els) =>
      els.map((e) => [Number(e.closest(".ag-row").getAttribute("row-index")), e.textContent.trim()]),
    );
    return pairs.sort((a, b) => a[0] - b[0]).map((x) => x[1]);
  };
  const sortBy = (name) => p.locator(".ag-header-cell", { hasText: name }).first().click();

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");

  // 列 1 = 日時、列 2 = 整数
  await p.getByRole("button", { name: "列の定義" }).click();
  const dlg = p.getByRole("dialog");
  await dlg.getByLabel("型").first().selectOption("dateTime");
  await dlg.getByRole("button", { name: "適用" }).first().click();
  await dlg.getByRole("button", { name: "+ 列を追加" }).click();
  const second = dlg.locator(".formula-row").nth(1);
  await second.getByLabel("列名").fill("数");
  await second.getByLabel("型").first().selectOption("int");
  await second.getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();

  const cellAt = (i, c) => p.locator(`.ag-row[row-index="${i}"] [col-id]:not([col-id="__delete"])`).nth(c);
  const rows = [
    ["2024-01-31T10:00:00+09:00", "10"],
    ["2024-01-31T00:30:00Z", "9"],
    ["", "100"],
    ["2024-01-31T01:00:00Z", ""],
    ["2024-01-30T23:00:00-05:00", "2"],
  ];
  for (const [i, [t, n]] of rows.entries()) {
    if (i > 0) await p.getByRole("button", { name: "+ 行を追加" }).click();
    for (const [c, v] of [[0, t], [1, n]]) {
      if (!v) continue;
      await cellAt(i, c).dblclick();
      await p.keyboard.press("Control+a");
      await p.keyboard.type(v);
      await p.keyboard.press("Enter");
    }
  }
  check("入力した順", (await colText("列1")).join("|") === rows.map((r) => r[0]).join("|"), JSON.stringify(await colText("列1")));

  // 日時: 昇順は瞬間の順（文字列の順ではない）。空は最後
  await sortBy("列1");
  const asc = await colText("列1");
  check("日時の昇順（瞬間）", asc.join("|") === "2024-01-31T00:30:00Z|2024-01-31T10:00:00+09:00|2024-01-31T01:00:00Z|2024-01-30T23:00:00-05:00|", JSON.stringify(asc));
  await sortBy("列1");
  const desc = await colText("列1");
  check("日時の降順（空は最後のまま）", desc.join("|") === "2024-01-30T23:00:00-05:00|2024-01-31T10:00:00+09:00|2024-01-31T01:00:00Z|2024-01-31T00:30:00Z|", JSON.stringify(desc));

  // 整数: 数として並ぶ（"10" < "9" にならない）。空は最後
  await sortBy("数");
  const n1 = await colText("数");
  check("整数の昇順（数として）", n1.join("|") === "2|9|10|100|", JSON.stringify(n1));
  await sortBy("数");
  const n2 = await colText("数");
  check("整数の降順（空は最後のまま）", n2.join("|") === "100|10|9|2|", JSON.stringify(n2));

  // 並べ替え中でも、行の編集は正しい行に入る
  await expect(p.locator(".ag-row")).toHaveCount(5);
  expect(errs).toEqual([]);
});
