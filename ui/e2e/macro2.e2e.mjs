import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1500, height: 800 } });

test("マクロの自動保存と保存・終了前の書き出し", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const setSource = (src) => p.evaluate((s) => { const m = window.monaco.editor.getModels().find((x) => x.uri.path.includes("macro-")); m.setValue(s); }, src);
  const editorText = () => p.evaluate(() => window.monaco.editor.getModels().find((x) => x.uri.path.includes("macro-"))?.getValue());
  const saveLabel = async () => (await p.getByRole("button", { name: /^保存/ }).first().innerText()).trim();
  const closed = () => p.evaluate(() => !!window.__closed);

  async function fresh() {
    await p.goto("/");
    await p.getByRole("button", { name: "新規作成" }).click();
    await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
    await p.waitForSelector(".ag-row");
    await p.getByRole("button", { name: "マクロ", exact: true }).click();
    await p.getByRole("button", { name: "+ 追加" }).click();
  await p.getByRole("menuitem", { name: "空のマクロ" }).click();
    await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
    await p.waitForSelector(".monaco-editor");
  }

  // 1. 入力した直後（自動保存の前）に保存しても、最後の編集がファイルに入る
  await fresh();
  const SRC1 = "export default function (jx) { return 'EDIT-1'; }\n";
  await setSource(SRC1);
  await p.getByRole("button", { name: /^保存/ }).first().click(); // 待たずに保存
  await p.waitForFunction(() => document.querySelector(".title")?.textContent?.includes("/mock/"));
  await p.waitForTimeout(900); // 自動保存のタイマーが残っていても、内容は同じなので未保存にならないはず
  check("入力直後に保存しても、保存後は未保存にならない", (await saveLabel()) === "保存", await saveLabel());
  // 開き直して、最後の編集が入っているか
  await p.getByRole("button", { name: "開く" }).click();
  await p.waitForSelector(".ag-row");
  await p.waitForTimeout(300);
  await p.waitForSelector(".monaco-editor"); // 開いた後もパネルは開いたまま
  await p.waitForTimeout(300);
  check("入力直後に保存した内容がファイルに入っている", (await editorText()) === SRC1, String(await editorText()));

  // 2. 入力した直後に終了しようとすると、未保存の警告が出る
  await fresh();
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForFunction(() => document.querySelector(".title")?.textContent?.includes("/mock/"));
  await p.waitForTimeout(300);
  await setSource("export default function (jx) { return 'EDIT-2'; }\n");
  await p.evaluate(() => window.__requestClose()); // 待たずに終了要求
  await p.getByRole("dialog").waitFor();
  check("入力直後の終了で警告が出る", (await p.getByRole("dialog").locator("h2").innerText()) === "終了前の確認");
  check("警告中は閉じない", !(await closed()));
  await p.getByRole("button", { name: "キャンセル" }).click();

  // 3. 入力した直後にパネルを閉じても、編集が失われない
  await fresh();
  const SRC3 = "export default function (jx) { return 'EDIT-3'; }\n";
  await setSource(SRC3);
  await p.getByRole("button", { name: "マクロ", exact: true }).click(); // パネルを閉じる
  await p.waitForTimeout(300);
  await p.getByRole("button", { name: "マクロ", exact: true }).click(); // 開き直す
  await p.waitForSelector(".monaco-editor");
  await p.waitForTimeout(500);
  check("パネルを閉じた直後の編集も残る", (await editorText()) === SRC3, String(await editorText()));
  check("パネルを閉じた直後の編集で未保存になる", (await saveLabel()) === "保存 *");


  expect(errs).toEqual([]);
});
