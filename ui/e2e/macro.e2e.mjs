import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1500, height: 800 } });

test("マクロエディタ", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const setSource = (src) => p.evaluate((s) => { const m = window.monaco.editor.getModels().find((x) => x.uri.path.includes("macro-")); m.setValue(s); }, src);
  const markers = () => p.evaluate(() => window.monaco.editor.getModelMarkers({}).map((m) => m.message));
  const saveLabel = async () => (await p.getByRole("button", { name: /^保存/ }).first().innerText()).trim();

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");
  check("新規直後は変更なし", (await saveLabel()) === "保存");

  // マクロ追加（テンプレートが入る）
  await p.getByRole("button", { name: "マクロ", exact: true }).click();
  await p.getByText("マクロがありません").waitFor();
  await p.getByRole("button", { name: "+ 追加" }).click();
  await p.getByRole("menuitem", { name: "空のマクロ" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".monaco-editor");
  check("マクロ追加で未保存になる", (await saveLabel()) === "保存 *");
  const tpl = await p.evaluate(() => window.monaco.editor.getModels().find((x) => x.uri.path.includes("macro-")).getValue());
  check("テンプレートが入っている", tpl.includes("export default"));

  // 型エラーの検出（jx の型定義が効いている）
  await setSource(`export default function (jx: Jxcel) {\n  jx.nope();\n  jx.sheet(1);\n}\n`);
  await p.waitForFunction(() => window.monaco.editor.getModelMarkers({}).length >= 2, null, { timeout: 15000 }).catch(() => {});
  const ms = await markers();
  check("存在しないメソッドを型エラーにする", ms.some((m) => m.includes("nope")), JSON.stringify(ms));
  check("引数の型違いを型エラーにする", ms.some((m) => /number|string/.test(m) && !m.includes("nope")), JSON.stringify(ms));

  // 型が正しいコードでは、エラーが消える
  await setSource(`export default function (jx: Jxcel) {\n  const s = jx.sheet("シート1").schema("データ");\n  s.rows().length;\n}\n`);
  await p.waitForFunction(() => window.monaco.editor.getModelMarkers({}).length === 0, null, { timeout: 15000 }).catch(() => {});
  check("正しいコードはエラーなし", (await markers()).length === 0, JSON.stringify(await markers()));

  // 実行（ブラウザ単体のモックは型注釈なしの JS のみ）
  await setSource(`export default function (jx) {\n  const s = jx.sheet("シート1").schema("データ");\n  s.add({ "列1": "マクロで追加" });\n  jx.log("追加しました");\n  return s.rows().length;\n}\n`);
  await p.getByRole("button", { name: "▶ 実行" }).click();
  await p.getByText("件の書き込みを反映しました").waitFor();
  const out = await p.locator(".macro-output").innerText();
  check("書き込み件数が出る", out.includes("1 件の書き込みを反映しました"), out);
  check("ログが出る", out.includes("追加しました"), out);
  check("戻り値が出る", out.includes("戻り値: 2"), out);
  check("グリッドに反映される", (await p.locator(".ag-row").count()) === 2 && (await p.locator(".grid-wrap").innerText()).includes("マクロで追加"));

  // 失敗するマクロは何も変えず、エラーが出る
  await setSource(`export default function (jx) {\n  const s = jx.sheet("シート1").schema("データ");\n  s.add({ "列1": "これは残らない" });\n  throw new Error("わざと失敗");\n}\n`);
  await p.getByRole("button", { name: "▶ 実行" }).click();
  await p.locator(".out-error").waitFor();
  check("エラーが表示される", (await p.locator(".out-error").innerText()).includes("わざと失敗"));
  check("失敗したマクロは何も残さない", (await p.locator(".ag-row").count()) === 2 && !(await p.locator(".grid-wrap").innerText()).includes("これは残らない"));

  // 書き込みのないマクロは未保存を増やさない（保存してから実行）
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(700);
  check("保存で未保存が消える", (await saveLabel()) === "保存");
  await setSource(`export default function (jx) { return jx.sheet("シート1").schema("データ").rows().length; }\n`);
  await p.waitForTimeout(900); // 自動保存（500ms）を待つ
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(500);
  await p.getByRole("button", { name: "▶ 実行" }).click();
  await p.getByText("書き込みはありませんでした").waitFor();
  check("書き込みなしの実行は未保存にしない", (await saveLabel()) === "保存");

  // 履歴にマクロの変更が出る
  await p.getByRole("button", { name: "履歴", exact: true }).click();
  await p.waitForSelector(".commits li");
  const texts = await p.locator(".changes ul li").allInnerTexts();
  check("履歴にマクロの変更が出る", texts.some((t) => t.includes("マクロ")), JSON.stringify(texts));


  expect(errs).toEqual([]);
});
