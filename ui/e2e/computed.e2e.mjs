import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1400, height: 800 } });

test("計算列", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const setFormula = (src) => p.evaluate((s) => { const m = window.monaco.editor.getModels().find((x) => x.uri.path.includes("formula-")); m.setValue(s); }, src);
  const gridText = () => p.locator(".grid-wrap").innerText();
  const cellTexts = async (colName) => {
    // 列名のヘッダから col-id を引き、その列のセルの文字を行順に返す
    const colId = await p.locator(".ag-header-cell", { hasText: colName }).first().getAttribute("col-id");
    return p.locator(`.ag-row [col-id="${colId}"]`).allInnerTexts();
  };

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");

  // 列1 を「数量（整数）」にして値を入れる
  await p.getByRole("button", { name: "列の定義" }).click();
  let dlg = p.getByRole("dialog");
  await dlg.getByLabel("列名").first().fill("数量");
  await dlg.getByLabel("型").first().selectOption("int");
  await dlg.getByRole("button", { name: "適用" }).first().click();
  // 計算列を追加
  await dlg.getByRole("button", { name: "+ 列を追加" }).click();
  const second = dlg.locator(".formula-row").nth(1);
  await second.getByLabel("列名").fill("倍");
  await second.getByLabel("型").first().selectOption("int");
  await second.getByLabel("計算列").check();
  await p.waitForSelector(".monaco-editor");
  await setFormula("export default function (row) { return row.数量 * 2; }\n");
  await second.getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();
  await p.waitForTimeout(300);

  check("計算列のヘッダに ƒ が付く", (await gridText()).includes("ƒ 倍"));

  // 元の列に値を入れると、計算列が再計算される
  const qty = p.locator('.ag-row[row-index="0"] [col-id]:not([col-id="__delete"])').first();
  await qty.dblclick();
  await p.keyboard.type("21");
  await p.keyboard.press("Enter");
  await p.waitForTimeout(300);
  check("数量=21 → 倍=42", (await cellTexts("倍"))[0] === "42", JSON.stringify(await cellTexts("倍")));

  // 計算列のセルは編集できない（エディタが開かない）
  const dbl = p.locator('.ag-row[row-index="0"] [col-id]').nth(2);
  await dbl.dblclick();
  await p.waitForTimeout(200);
  check("計算列のセルは編集できない", (await p.locator(".ag-cell-inline-editing, .ag-cell-edit-wrapper").count()) === 0);
  check("計算列のセルは computed 表示", ((await dbl.getAttribute("class")) ?? "").includes("computed"));

  // 行を追加 → 数量が空なので 0
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  await p.waitForTimeout(300);
  check("空の行は 0 になる", (await cellTexts("倍")).join(",") === "42,0", JSON.stringify(await cellTexts("倍")));

  // 型に合わない結果は、そのセルだけ #ERROR
  await p.getByRole("button", { name: "列の定義" }).click();
  dlg = p.getByRole("dialog");
  await setFormula("export default function (row) { return row.数量 === null ? 'なし' : row.数量 * 2; }\n");
  await dlg.locator(".formula-row").nth(1).getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();
  await p.waitForTimeout(300);
  const vals = await cellTexts("倍");
  check("型違いのセルだけ #ERROR（他は計算される）", vals.join(",") === "42,#ERROR", JSON.stringify(vals));
  check("エラーセルに cell-error", (await p.locator(".cell-error").count()) === 1);

  // 式の構文エラーはその列の全セルが #ERROR（表は壊れない）
  await p.getByRole("button", { name: "列の定義" }).click();
  dlg = p.getByRole("dialog");
  await setFormula("export default function (row) { return row.数量 * ;\n");
  await dlg.locator(".formula-row").nth(1).getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();
  await p.waitForTimeout(300);
  check("構文エラーの式は全セル #ERROR", (await cellTexts("倍")).every((t) => t === "#ERROR"), JSON.stringify(await cellTexts("倍")));
  check("他の列は無事", (await cellTexts("数量"))[0] === "21");

  // 正しい式に戻して保存 → 履歴に「計算式を変更」
  await p.getByRole("button", { name: "列の定義" }).click();
  dlg = p.getByRole("dialog");
  await setFormula("export default function (row) { return row.数量 * 3; }\n");
  await dlg.locator(".formula-row").nth(1).getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();
  await p.waitForTimeout(300);
  check("式を直すと再計算される", (await cellTexts("倍"))[0] === "63", JSON.stringify(await cellTexts("倍")));
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(400);
  await p.getByRole("button", { name: "列の定義" }).click();
  dlg = p.getByRole("dialog");
  await setFormula("export default function (row) { return row.数量 * 4; }\n");
  await dlg.locator(".formula-row").nth(1).getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(400);
  await p.getByRole("button", { name: "履歴", exact: true }).click();
  await p.waitForSelector(".commits li");
  const changes = await p.locator(".changes ul li").allInnerTexts();
  check("履歴に「計算式を変更」が出る", changes.some((t) => t.includes("計算式を変更")), JSON.stringify(changes));

  // 開き直しても式が残り、値は計算し直される
  await p.getByRole("button", { name: "開く" }).click();
  await p.waitForSelector(".ag-row");
  await p.waitForTimeout(300);
  check("開き直しても計算される（×4）", (await cellTexts("倍"))[0] === "84", JSON.stringify(await cellTexts("倍")));


  expect(errs).toEqual([]);
});
