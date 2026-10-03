import { test, expect } from "@playwright/test";

test.use({ viewport: { width: 1500, height: 850 }, permissions: ["clipboard-read", "clipboard-write"] });

test("フォーム配信のパネル", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  // 検査 1 件ごとに名前を付け、失敗しても残りを続けて検査する
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const panel = () => p.getByRole("complementary", { name: "フォーム配信" });

  await p.goto("/");
  await p.getByRole("button", { name: "新規作成" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await p.waitForSelector(".ag-row");

  // 列を用意する: 列1 → 氏名（必須）、数量（整数）、倍（計算列）
  await p.getByRole("button", { name: "列の定義" }).click();
  let dlg = p.getByRole("dialog");
  await dlg.getByLabel("列名").first().fill("氏名");
  await dlg.getByLabel("必須").first().check();
  await dlg.getByRole("button", { name: "適用" }).first().click();
  await dlg.getByRole("button", { name: "+ 列を追加" }).click();
  let row2 = dlg.locator(".formula-row").nth(1);
  await row2.getByLabel("列名").fill("数量");
  await row2.getByLabel("型").first().selectOption("int");
  await row2.getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "+ 列を追加" }).click();
  let row3 = dlg.locator(".formula-row").nth(2);
  await row3.getByLabel("列名").fill("倍");
  await row3.getByLabel("型").first().selectOption("int");
  await row3.getByLabel("計算列").check();
  await p.waitForSelector(".monaco-editor");
  await row3.getByRole("button", { name: "適用" }).click();
  await dlg.getByRole("button", { name: "閉じる" }).click();
  await p.waitForTimeout(300);

  await p.getByRole("button", { name: "フォーム", exact: true }).click();
  check("フォームがないと説明が出る", (await panel().innerText()).includes("フォームにすると"));
  check("フォームがないと配信は始められない", await panel().getByRole("button", { name: "配信を開始" }).isDisabled());
  check("停止中と出る", (await panel().innerText()).includes("停止中"));

  await panel().getByRole("button", { name: "+ フォームを追加" }).click();
  await panel().getByLabel("フォームの名前").waitFor();
  check("名前の既定は『表名の入力』", (await panel().getByLabel("フォームの名前").inputValue()).endsWith("の入力"));
  const checks = panel().locator(".field-row input[type=checkbox]");
  check("入力欄は計算列を除く 2 列", (await checks.count()) === 2 && (await checks.evaluateAll((l) => l.every((e) => e.checked))));
  check("計算列は候補に出ない", !(await panel().innerText()).includes("倍"));
  check("未適用の変更はない", await panel().getByRole("button", { name: "適用" }).isDisabled());

  // 編集 → 適用
  await panel().getByLabel("フォームの名前").fill("来客受付");
  await panel().getByLabel("説明").fill("お名前を入力してください");
  await panel().getByRole("button", { name: "数量 を上へ" }).click();
  check("入力欄を並べ替えると適用が有効", await panel().getByRole("button", { name: "適用" }).isEnabled());
  await panel().getByRole("button", { name: "適用" }).click();
  await p.waitForFunction(() => document.querySelector(".field-row label")?.textContent?.startsWith("数量"));
  check("並び順が反映される（数量が先頭）", (await panel().locator(".field-row label").first().innerText()).startsWith("数量"));
  check("名前が反映される", (await panel().getByLabel("フォーム", { exact: true }).innerText()).includes("来客受付"));

  // 全部外すと拒否される
  await checks.nth(0).uncheck();
  await checks.nth(0).uncheck();
  await panel().getByRole("button", { name: "適用" }).click();
  await p.waitForFunction(() => document.body.innerText.includes("1 つ以上選んでください"), null, { timeout: 3000 }).catch(() => {});
  check("入力欄ゼロは拒否される", (await p.locator("body").innerText()).includes("1 つ以上選んでください"));
  // 入力欄を戻して適用
  await p.locator(".fields input[type=checkbox]").nth(0).check();
  await p.locator(".fields input[type=checkbox]").nth(1).check();
  await panel().getByRole("button", { name: "適用" }).click();
  await p.waitForFunction(() => document.querySelectorAll(".field-row input:checked").length === 2);

  // 送信後の修正の設定（変えるとすぐ反映。下書きは捨てない）
  const allowBox = panel().getByLabel("回答者が送信後に修正できる");
  const minutesBox = panel().getByLabel("修正できる期間（分）");
  check("修正は既定で許す・期限なし", (await allowBox.isChecked()) && (await minutesBox.inputValue()) === "");
  await panel().getByLabel("説明").fill("書きかけの説明");
  await minutesBox.fill("60");
  await minutesBox.blur();
  await p.waitForTimeout(150);
  check("期間を入れても入力欄の下書きは消えない", (await panel().getByLabel("説明").inputValue()) === "書きかけの説明");
  check("期間が反映される", (await minutesBox.inputValue()) === "60");
  await minutesBox.fill("0");
  await minutesBox.press("Enter");
  await p.waitForFunction(() => document.body.innerText.includes("1〜43200 分"), null, { timeout: 3000 }).catch(() => {});
  check("0 分は拒否され、元の値に戻る", (await p.locator("body").innerText()).includes("1〜43200 分") && (await minutesBox.inputValue()) === "60");
  await allowBox.uncheck();
  await p.waitForFunction(() => !document.querySelector('[aria-label="修正できる期間（分）"]'));
  check("許さないと期間の欄は隠れる", (await minutesBox.count()) === 0);
  check("設定を変えても下書きは消えない", (await panel().getByLabel("説明").inputValue()) === "書きかけの説明");
  await allowBox.check();
  check("許すに戻すと、前の期間が残っている", (await minutesBox.inputValue()) === "60");
  await minutesBox.fill("");
  await minutesBox.blur();
  await p.waitForTimeout(150);
  check("空にすると期限なし", (await minutesBox.inputValue()) === "");
  await panel().getByLabel("説明").fill("お名前を入力してください");

  // 回答者ごとの合言葉: 規則に合わないもの・重複は開始できない
  const personalBox = panel().getByLabel("回答者ごとの合言葉");
  await personalBox.fill("abc");
  await panel().getByRole("button", { name: "配信を開始" }).click();
  await p.waitForFunction(() => document.body.innerText.includes("合言葉は半角"), null, { timeout: 3000 }).catch(() => {});
  check("短い合言葉は拒否される", (await p.locator("body").innerText()).includes("合言葉は半角") && (await panel().getByRole("button", { name: "配信を開始" }).isVisible()));
  await personalBox.fill("yamada-1\nyamada-1");
  await panel().getByRole("button", { name: "配信を開始" }).click();
  await p.waitForFunction(() => document.body.innerText.includes("重複"), null, { timeout: 3000 }).catch(() => {});
  check("重複は拒否される", (await p.locator("body").innerText()).includes("重複"));
  await personalBox.fill("");

  // 配信の開始
  check("必須の列を外す前は警告なし", true);
  await panel().getByRole("button", { name: "配信を開始" }).click();
  await panel().getByLabel("来客受付 の URL").waitFor();
  const url = await panel().getByLabel("来客受付 の URL").inputValue();
  check("URL が出る（/f/ と 32 桁トークン）", /^http:\/\/[\d.]+:8787\/f\/[0-9a-z]{32}$/.test(url), url);
  check("配信中と出る", (await panel().innerText()).includes("配信中（ポート 8787）"));
  check("配信中はポートを変えられない", await panel().getByLabel("ポート").isDisabled());
  check("回答 0 件", (await panel().innerText()).includes("回答 0 件"));
  await panel().getByRole("button", { name: "コピー" }).click();
  await p.waitForTimeout(150);
  check("コピーできる", (await p.evaluate(() => navigator.clipboard.readText())) === url);
  const rows0 = await p.locator(".ag-row").count();

  // 回答が届く → 表が自動で更新され、回答数も増え、未保存になる
  await p.evaluate(() => window.__mockSubmit("来客受付", { 氏名: "山田", 数量: "3" }));
  await p.waitForFunction((n) => document.querySelectorAll(".ag-row").length === n + 1, rows0);
  check("回答が行として現れる", (await p.locator(".ag-row").count()) === rows0 + 1 && (await p.locator(".ag-root").innerText()).includes("山田"));
  check("計算列も計算される", (await p.locator(".ag-root").innerText()).includes("山田"));
  await p.waitForFunction(() => document.body.innerText.includes("回答 1 件"));
  check("回答数が増える", true);
  check("未保存になる", (await p.getByRole("button", { name: /^保存/ }).first().innerText()).includes("*"));
  let msg = await p.evaluate(() => { try { window.__mockSubmit("来客受付", { 倍: 1 }); return "no error"; } catch (e) { return String(e); } });
  check("フォームにない欄は拒否される", msg.includes("フォームにない欄"), msg);

  // 停止
  await panel().getByRole("button", { name: "配信を停止" }).click();
  await panel().getByRole("button", { name: "配信を開始" }).waitFor();
  check("停止すると URL が消える", (await panel().getByLabel("来客受付 の URL").count()) === 0);

  // 履歴の差分にフォームの追加が出る
  await p.getByRole("button", { name: /^保存/ }).first().click();
  await p.waitForTimeout(300);

  // フォームの削除
  await panel().getByRole("button", { name: "削除" }).click();
  await p.getByRole("dialog").getByRole("button", { name: "OK" }).click();
  await panel().getByText("フォームにすると").waitFor();
  check("削除するとフォームがなくなる", (await panel().innerText()).includes("フォームにすると"));

  expect(errs).toEqual([]);
});

// 複数ファイルの同時配信: 配信を続けたまま別のファイルを開くと、元のファイルは裏で配信し続ける
test("フォーム配信: 配信を続けたまま別のファイルを開く", async ({ page: p }) => {
  const errs = [];
  p.on("pageerror", (e) => errs.push("pageerror: " + e.message));
  const check = (name, cond, extra = "") => expect.soft(!!cond, name + (cond ? "" : " " + extra)).toBe(true);
  const panel = () => p.getByRole("complementary", { name: "フォーム配信" });
  const newFile = async (name) => {
    // ファイルが開いていないときは「新規作成」、開いているときはツールバーの「新規」
    await p.getByRole("button", { name: /^新規(作成)?$/ }).first().click();
    const dlg = p.getByRole("dialog");
    await dlg.getByRole("textbox").fill(name);
    await dlg.getByRole("button", { name: "OK" }).click();
    await p.waitForSelector(".ag-row");
  };
  const save = async () => {
    await p.getByRole("button", { name: /^保存/ }).first().click();
    await p.waitForFunction(() => !/保存\s*\*/.test(document.body.innerText.split("\n").find((l) => l.startsWith("保存")) ?? ""));
  };

  await p.goto("/");
  // ファイル B を作って保存する（あとで開く側）
  await newFile("ファイルB");
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  await save();
  // ファイル A: フォームを作って保存し、配信を始める
  await newFile("ファイルA");
  await p.getByRole("button", { name: "フォーム", exact: true }).click();
  await panel().getByRole("button", { name: "+ フォームを追加" }).click();
  await panel().getByLabel("フォームの名前").waitFor();
  await save();
  await panel().getByRole("button", { name: "配信を開始" }).click();
  await panel().getByLabel("データの入力 の URL").waitFor();

  // 保存していないと裏には回せない
  await p.getByRole("button", { name: "+ 行を追加" }).click();
  await panel().getByRole("button", { name: "配信を続けたまま、別のファイルを開く…" }).click();
  await p.waitForFunction(() => document.body.innerText.includes("未保存の変更があります"), null, { timeout: 3000 }).catch(() => {});
  check("未保存のファイルは裏に回せない", (await p.locator("body").innerText()).includes("未保存の変更があります"));
  await save();

  // 保存済みなら開ける。ファイル A のフォームは配信され続ける
  await panel().getByRole("button", { name: "配信を続けたまま、別のファイルを開く…" }).click();
  await panel().getByLabel("裏で配信しているファイル").waitFor();
  check("裏で配信しているファイルが一覧に出る", (await panel().getByLabel("裏で配信しているファイル").innerText()).includes("ファイルA"));
  check("元のフォームの URL は残り、別のファイルと分かる", (await panel().getByLabel("配信中の URL").innerText()).includes("別のファイル「ファイルA」"));
  check("開いたのはファイル B（フォームなし）", (await panel().innerText()).includes("フォームにすると"));
  check("配信は続いている", (await panel().innerText()).includes("配信中"));

  // 裏のファイルへの回答は届き、回答数が増える
  await p.evaluate(() => window.__mockSubmit("データの入力", { 列1: "山田" }));
  await p.waitForFunction(() => document.body.innerText.includes("回答 1 件"));
  check("裏のファイルの回答数が増える", true);

  // 同じファイルをもう一度は開けない（開いているのは B で、A は配信中）。配信をやめると一覧から消える
  await panel().getByRole("button", { name: "ファイルA の配信をやめる" }).click();
  await p.waitForFunction(() => !document.querySelector('[aria-label="裏で配信しているファイル"]'));
  check("配信をやめると一覧と URL から消える", (await panel().getByLabel("配信中の URL").innerText()).trim() === "");

  expect(errs).toEqual([]);
});
