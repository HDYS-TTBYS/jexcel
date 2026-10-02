import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1500, height: 850 } });

test("標準ライブラリとサンプルマクロ", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  // 表示中のエディタ（Monaco は編集済みのモデルを残すので、モデル一覧からは探さない）
  const macroText = () => p.evaluate(() => window.monaco.editor.getEditors().find((e) => e.getModel()?.uri.path.includes("macro-"))?.getModel().getValue());
  const setSource = (src, kind = "macro-") => p.evaluate(([s, k]) => window.monaco.editor.getEditors().find((e) => e.getModel()?.uri.path.includes(k)).getModel().setValue(s), [src, kind]);

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");
  await p.getByRole("button", { name: "マクロ", exact: true }).click();

  // 追加メニューにサンプルが並ぶ
  await p.getByRole("button", { name: "+ 追加" }).click();
  await p.getByRole("menuitem", { name: "空のマクロ" }).waitFor();
  const items = await p.getByRole("menuitem").allInnerTexts();
  check("メニューに空のマクロと 5 つのサンプル", items.length === 6, String(items.length));
  check("サンプル名と説明が出る", items.some((t) => t.includes("表記ゆれの整理") && t.includes("全角英数")));

  // サンプルから追加: 名前の既定はサンプル名、ソースはサンプルの内容（@name / @desc の行は無い）
  await p.getByRole("menuitem", { name: /重複行の削除/ }).click();
  const dlg = p.getByRole("dialog");
  check("名前の既定がサンプル名", (await dlg.locator("input").inputValue()) === "重複行の削除");
  await dlg.getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".monaco-editor");
  await p.waitForTimeout(500);
  const src = await macroText();
  check("サンプルのソースが入る", src.includes("const KEYS") && src.includes("schema.remove(row._id)"), src.slice(0, 80));
  check("@name / @desc の行は入らない", !src.includes("@name") && !src.includes("@desc"));
  check("説明が先頭のコメントになる", src.startsWith("// 指定した列の組が同じ行のうち"));

  // std をマクロで使う（ブラウザ単体は型注釈なしの JS のみ実行できる）
  await p.getByRole("button", { name: "+ 追加" }).click();
  await p.getByRole("menuitem", { name: "空のマクロ" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForTimeout(500);
  await setSource(`export default function (jx) {\n  return [std.dec.add("0.1", "0.2"), std.sum([1, 2, null]), std.date.addMonths("2024-01-31", 1), std.text.normalize("Ａ　ｂ")];\n}\n`);
  await p.getByRole("button", { name: "▶ 実行" }).click();
  await p.locator(".out-result").waitFor();
  const raw = await p.locator(".out-result").innerText();
  const out = JSON.parse(raw.replace("戻り値:", "").trim()); // 実際の戻り値をそのまま比較する（空白を削らない）
  check("マクロから std が使える", JSON.stringify(out) === JSON.stringify(["0.3", 3, "2024-02-29", "A b"]), raw);

  // std の型が効く（補完用の型定義: 存在しない関数はエラー）
  await setSource(`export default function (jx: Jxcel) {\n  std.nope();\n  const n: number = std.text.trim("a");\n  return std.date.addDays("2024-01-01", "x");\n}\n`);
  await p.waitForFunction(() => window.monaco.editor.getModelMarkers({}).length >= 3, null, { timeout: 20000 }).catch(() => {});
  const ms = await p.evaluate(() => window.monaco.editor.getModelMarkers({}).map((m) => m.message));
  check("std の型エラーを検出（存在しない関数）", ms.some((m) => m.includes("nope")), JSON.stringify(ms));
  check("std の型エラーを検出（戻り値の型違い）", ms.some((m) => /string/.test(m) && /number/.test(m)), JSON.stringify(ms));
  check("std の型エラーを検出（引数の型違い）", ms.some((m) => /number/.test(m) && /string/.test(m)) && ms.length >= 3, JSON.stringify(ms));

  // ホバーで日本語の説明が出る（JSDoc）
  await setSource(`export default function (jx: Jxcel) {\n  return std.date.addMonths("2024-01-31", 1);\n}\n`);
  const doc = await p.evaluate(async () => {
    const m = window.monaco;
    const model = m.editor.getEditors().find((e) => e.getModel()?.uri.path.includes("macro-")).getModel();
    const worker = await (await m.typescript.getTypeScriptWorker())(model.uri);
    const offset = model.getValue().indexOf("addMonths") + 2;
    const info = await worker.getQuickInfoAtPosition(model.uri.toString(), offset);
    return (info?.documentation ?? []).map((d) => d.text).join("");
  });
  check("ホバーに日本語の説明が出る（月末の丸め）", doc.includes("月末"), doc);

  // 計算列の式でも std が使える
  await p.getByRole("button", { name: "列の定義" }).click();
  const d2 = p.getByRole("dialog");
  await d2.getByLabel("列名").first().fill("名前");
  await d2.getByRole("button", { name: "適用" }).first().click();
  await d2.getByRole("button", { name: "+ 列を追加" }).click();
  const second = d2.locator(".formula-row").nth(1);
  await second.getByLabel("列名").fill("整形");
  await second.getByLabel("計算列").check();
  await p.waitForFunction(() => window.monaco && window.monaco.editor.getEditors().some((e) => e.getModel()?.uri.path.includes("formula-")));
  await p.waitForTimeout(800); // エディタの初期化が終わってから値を入れる（直後だと、初期化が元の値で上書きする）
  await setSource(`export default function (row) { return std.text.normalize(row.名前) + "!"; }\n`, "formula-");
  await second.getByRole("button", { name: "適用" }).click();
  await d2.getByRole("button", { name: "閉じる" }).click();
  const cell = p.locator('.ag-row[row-index="0"] [col-id]:not([col-id="__delete"])').first();
  await cell.dblclick();
  await p.keyboard.type("  Ａｂｃ　１２３ ");
  await p.keyboard.press("Enter");
  await p.waitForTimeout(400);
  const texts = await p.locator(".ag-row[row-index='0'] [col-id]").allInnerTexts();
  check("計算列の式から std が使える", texts.includes("Abc 123!"), JSON.stringify(texts));


  expect(errs).toEqual([]);
});
