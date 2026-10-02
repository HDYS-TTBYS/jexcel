import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1500, height: 850 } });

test("テンプレート書き出しのパネル", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const panel = () => p.getByRole("complementary", { name: "書き出し" });
  const previewRows = async () => (await panel().locator(".preview tbody tr").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
  const applyBtn = () => panel().getByRole("button", { name: "適用" });

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");

  // データ: 列1 に 3 行
  const cell = (i) => p.locator(`.ag-row[row-index="${i}"] [col-id]:not([col-id="__delete"])`).first();
  await cell(0).dblclick(); await p.keyboard.type("A社"); await p.keyboard.press("Enter");
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  await cell(1).dblclick(); await p.keyboard.type("B商事"); await p.keyboard.press("Enter");
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  await cell(2).dblclick(); await p.keyboard.type("A社"); await p.keyboard.press("Enter");
  await p.waitForTimeout(300);

  // 空の状態
  await p.getByRole("button", { name: "書き出し", exact: true }).click();
  check("書き出しがないときの説明", (await panel().innerText()).includes("テンプレートを取り込むと"));

  // テンプレートを追加（モックは /mock/請求書.docx を返す）
  await panel().getByRole("button", { name: "+ テンプレートを追加" }).click();
  await panel().getByLabel("書き出しの名前").waitFor();
  check("名前の既定はテンプレートのファイル名", (await panel().getByLabel("書き出しの名前").inputValue()) === "請求書");
  check("テンプレート名と種類が出る", (await panel().innerText()).includes("請求書.docx"));
  check("出力ファイル名の既定は先頭の列", (await panel().getByLabel("出力ファイル名").inputValue()) === "{{列1}}");
  await panel().locator(".preview tbody tr").first().waitFor();
  let rows = await previewRows();
  check("プレビューに全行のファイル名", rows.length === 3 && rows[0].includes("A社.docx") && rows[1].includes("B商事.docx"), JSON.stringify(rows));
  check("同じ名前は (2) が付く", rows[2].includes("A社 (2).docx"), JSON.stringify(rows));
  check("未適用の変更はない", await applyBtn().isDisabled());

  // std と行番号をファイル名に使う → 適用でプレビューが変わる
  await panel().getByLabel("出力ファイル名").fill("{{ std.text.zeroPad(_no, 3) }}_{{列1}}");
  check("編集すると「適用」が有効", await applyBtn().isEnabled());
  check("未適用の間は書き出せない", await panel().getByRole("button", { name: "書き出す…" }).isDisabled());
  await applyBtn().click();
  await p.waitForFunction(() => document.querySelector(".preview tbody tr td:nth-child(2)")?.textContent?.startsWith("001_"));
  rows = await previewRows();
  check("std・_no を使ったファイル名", rows[0].includes("001_A社.docx") && rows[2].includes("003_A社.docx"), JSON.stringify(rows));

  // 絞り込み: 除外される行は取り消し線
  await panel().getByLabel("絞り込み").fill('列1 === "A社"');
  await applyBtn().click();
  await p.waitForSelector(".preview tr.excluded");
  check("絞り込みで除外される行が分かる", (await p.locator(".preview tr.excluded").count()) === 1);
  check("除外の件数が出る", (await panel().innerText()).includes("1 行は絞り込みで除外") || (await panel().innerText()).includes("1 行を絞り込みで除外") || (await panel().innerText()).includes("の 1 行"));

  // ファイル名の式が失敗する → その行のエラー（パネルは壊れない）
  await panel().getByLabel("絞り込み").fill("");
  await panel().getByLabel("出力ファイル名").fill("{{列1.foo.bar}}");
  await applyBtn().click();
  await p.waitForSelector(".preview td.cell-error");
  check("式の失敗は #ERROR で表示される", (await p.locator(".preview td.cell-error").count()) >= 1);
  const title = await p.locator(".preview td.cell-error").first().getAttribute("title");
  check("エラーの理由（式）がツールチップに入る", title.includes("列1.foo.bar") || title.includes("ファイル名"), title);

  // 書き出し: ブラウザ単体は書けないのでエラーが出る
  await panel().getByLabel("出力ファイル名").fill("{{列1}}");
  await applyBtn().click();
  await p.waitForTimeout(300);
  await panel().getByRole("button", { name: "書き出す…" }).click();
  await p.getByRole("alert").waitFor();
  check("ブラウザ単体では書き出せない旨が出る", (await p.getByRole("alert").innerText()).includes("デスクトップ版"));

  // 保存 → 履歴に「書き出し」の追加が出る
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(400);
  await p.getByRole("button", { name: "履歴", exact: true }).click();
  await p.waitForSelector(".commits li");
  const changes = await p.locator(".changes ul li").allInnerTexts();
  // 初回の保存なので差分は無いが、2 回目の保存で出る: もう一度保存するために設定を変える
  await panel().getByLabel("出力ファイル名").fill("{{列1}}_x");
  await applyBtn().click();
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(400);
  await p.waitForFunction(() => document.querySelectorAll(".commits li").length >= 2);
  const changes2 = await p.locator(".changes ul li").allInnerTexts();
  check("履歴に書き出し設定の変更が出る", changes2.some((t) => t.includes("書き出し") && t.includes("設定を変更")), JSON.stringify(changes2));

  // 削除（確認ダイアログ）
  await panel().getByRole("button", { name: "削除", exact: true }).click();
  check("削除の確認が出る", (await p.getByRole("dialog").locator("h2").innerText()) === "書き出しの削除");
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForTimeout(300);
  check("削除すると空の状態に戻る", (await panel().innerText()).includes("テンプレートを取り込むと"));


  expect(errs).toEqual([]);
});
